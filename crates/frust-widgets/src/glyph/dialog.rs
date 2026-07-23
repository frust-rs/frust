//! Glyph modal dialog (task 26, glyph-design-system): a scrim + centered
//! surface panel confirmation dialog — the Glyph recipe over the existing
//! navigator transparent-push modal plumbing.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! Like [`crate::material::dialog`], a `GlyphDialog` is designed to be pushed
//! as a **transparent navigator page** via
//! [`NavigatorController::push_transparent_for_result`] — the page below stays
//! visible under the scrim, and the navigator's existing modal contract
//! (input routed only to the top page, pointer capture / focus / IME threaded
//! through the page pod) is what makes it modal. [`show_glyph_dialog`] wraps
//! the push and wires dismissal to `controller.pop()`. This module edits **no**
//! nav file: it consumes `push_transparent_for_result`/`pop` read-only.
//!
//! # Enter/exit staging (widget-internal, not a navigator transition)
//!
//! Unlike the material dialog — which rides a whole-page
//! [`PageTransition::M3FadeThrough`] — the Glyph dialog drives its **own**
//! enter/exit animation from `PaintCtx::frame_time`, because the two moving
//! parts stage *independently*: the **panel** scales `0.94 → 1.0` over the
//! spatial `enter` duration (220ms) while the **scrim** cross-fades on its own
//! clamped progress. A single navigator [`Layer`](crate::nav::transition::Layer)
//! (one alpha + one scale over the whole page) cannot express "panel scales but
//! scrim only fades", so the staging lives here (the toast precedent —
//! `crate::glyph::toast` — drives its own timeline the same way). The dialog is
//! therefore pushed with [`TransitionSpec::NONE`] (the navigator gives the modal
//! contract; the widget gives the motion).
//!
//! Exit reverses over the faster `exit` duration (150ms) — "exits always faster
//! than entrances" (RESEARCH §1.4). A dismiss gesture does **not** pop
//! immediately: it flips the widget into its exit phase, and only when the exit
//! animation completes is the app-supplied close callback ([`on_close`] /
//! [`show_glyph_dialog`]'s auto-wired `controller.pop()`) fired — from paint,
//! which is sound because `NavigatorController::pop` merely enqueues an op
//! applied on the next rebuild (see [`GlyphDialogWidget::advance`]).
//!
//! [`on_close`]: GlyphDialogView::on_close
//!
//! # The dismiss callback is state-free (a deliberate divergence)
//!
//! Because the close callback fires from **paint** (after the exit animation),
//! it is a plain `Fn()` — not the material dialog's `Fn(&mut State)`. App state
//! changes flow through the navigator's `on_result` callback instead (delivered
//! with `&mut State` after the pop, carrying the dialog's [`PopResult`]): an
//! **action** button pops with a value (`controller.pop_with_result(..)`), a
//! **scrim/Escape cancel** pops empty. This split — "close me" is state-free,
//! "here's my answer" rides `on_result` — is the whole point of the
//! result-callback shape.
//!
//! # Actions
//!
//! Action buttons are app-provided [`AnyView`]s (task 22 button styles: a ghost
//! cancel + a danger/primary confirm). They pop the navigator **directly** on
//! tap (e.g. `controller.pop_with_result(PopResult::of(true))`), bypassing the
//! widget's own exit animation — the same immediacy the material dialog's
//! actions have. Only the scrim-tap / Escape *cancel* path plays the exit
//! staging.
//!
//! # Only one floating layer
//!
//! Enforcing "at most one dialog/overlay at a time" is the app's (or the
//! navigator's) concern — this widget does not police it. Pushing a second
//! transparent page simply stacks another modal on top.
//!
//! # Semantics
//!
//! The whole dialog contributes one [`Role::Dialog`] container node with the
//! accesskit **modal** flag set, labelled by the title; title/body/actions are
//! its accesskit children (mirroring [`crate::material::dialog`]).

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase,
    SemanticsCtx, View, Widget, any,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, LineHeight};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::Timing;
use crate::nav::navigator::{NavigatorController, PopResult};
use crate::nav::transition::{TransitionDriver, TransitionSpec, make_driver};
use crate::text::{ThemeTextColor, text};

/// Scrim opacity behind the panel at full enter (this task's own choice — a
/// terminal-dark modal barrier, heavier than M3's 32% so the amber panel reads
/// against the void surface).
const SCRIM_ALPHA: f32 = 0.6;
/// Panel padding on all four edges, in logical px (this task's own choice —
/// Glyph's roomy `--space-5` gutter).
const DIALOG_PADDING: f64 = 20.0;
/// Vertical spacing between the title and body, in logical px.
const TITLE_BODY_GAP: f64 = 10.0;
/// Vertical spacing between the body and the action row, in logical px.
const BODY_ACTIONS_GAP: f64 = 20.0;
/// Horizontal spacing between adjacent action buttons, in logical px.
const ACTION_GAP: f64 = 10.0;
/// Minimum dialog panel width, in logical px.
const MIN_WIDTH: f64 = 300.0;
/// Maximum dialog panel width, in logical px.
const MAX_WIDTH: f64 = 480.0;

/// Unthemed-fallback panel corner radius — Glyph's `--radius-lg` (14px). A theme
/// resolves this from `shape.large`.
const RADIUS: f64 = 14.0;
/// Unthemed-fallback panel surface fill (Glyph dark `bg-raised` `#1e2430`). A
/// theme resolves this from `colors.surface_container_high`.
const CONTAINER: Color = Color::from_rgb8(0x1e, 0x24, 0x30);
/// Unthemed-fallback panel border (Glyph dark `border-bright`). A theme resolves
/// this from `colors.outline`.
const BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Panel border width, in logical px (`--border` = 1px).
const BORDER_WIDTH: f64 = 1.0;
/// Corner-rounding tolerance for the border stroke (mirrors
/// `crate::glyph::alert`'s constant).
const PATH_TOLERANCE: f64 = 0.1;
/// Unthemed-fallback scrim base color (a theme resolves this from `colors.scrim`).
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback chrome-level shadow (Glyph `--shadow-chrome`: a deep
/// `0 12px 32px` drop). A theme resolves this from `elevation.level5`.
const SHADOW_Y: f64 = 12.0;
const SHADOW_BLUR: f64 = 32.0;
const SHADOW_ALPHA: f32 = 0.45;

/// Title type role: Glyph `display` family (Space Mono) at 15/700 (task detail).
const TITLE_SIZE: f32 = 15.0;
const TITLE_WEIGHT: FontWeight = FontWeight::BOLD;
const TITLE_LINE_HEIGHT: f32 = 20.0;
/// Body type role: Glyph `body` family (IBM Plex Mono) at 12.5, `fg-muted`.
const BODY_SIZE: f32 = 12.5;
const BODY_LINE_HEIGHT: f32 = 18.0;

/// Panel enter scale start (`0.94 → 1.0` over the spatial enter — task detail).
const ENTER_SCALE_START: f64 = 0.94;
/// Enter duration fallback (`durations.base` = 220ms) when no theme is threaded.
const ENTER_DURATION: Duration = Duration::from_millis(220);
/// Enter easing fallback (Glyph `spatial` cubic — an authored overshoot).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration fallback (`durations.fast` = 150ms; exits faster than entrances).
const EXIT_DURATION: Duration = Duration::from_millis(150);
/// Exit easing fallback (Glyph `exit` accelerate-out cubic).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors
/// `nav::transition::REDUCE_MOTION_DURATION`).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Replace `color`'s alpha channel with `alpha`.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state: the
/// authored spatial/exit durations+curves, or both collapsed to a fast linear
/// crossfade (RESEARCH §1.4's hard accessibility rule).
fn resolve_timings(theme: Option<&Theme>) -> (Timing, Timing) {
    if theme.map(|t| t.motion.reduce_motion).unwrap_or(false) {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        return (t, t);
    }
    match theme {
        Some(t) => (
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.base / 1000.0),
                t.motion.easing.spatial,
            ),
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.fast / 1000.0),
                t.motion.easing.exit,
            ),
        ),
        None => (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        ),
    }
}

fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_high,
        None => CONTAINER,
    }
}

fn resolve_border(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().outline,
        None => BORDER,
    }
}

fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(t) => t.shape.large,
        None => RADIUS,
    }
}

fn resolve_scrim(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().scrim,
        None => SCRIM,
    }
}

/// The `(y_offset, blur_std_dev, color)` chrome-level shadow triple: themed
/// `elevation.level5.shadow(brightness)`, or the unthemed fallback.
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(t) => {
            let s = t.elevation.level5.shadow(t.brightness);
            (
                s.y_offset,
                s.blur_std_dev,
                with_alpha(t.scheme().shadow, s.color_alpha),
            )
        }
        None => (SHADOW_Y, SHADOW_BLUR, with_alpha(SCRIM, SHADOW_ALPHA)),
    }
}

/// Coerce a possibly-infinite constraint dimension to a finite value.
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The title child view (Glyph display family, 15/700, `onSurface`).
fn title_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .family(FontFamily::stack_with_generic(
                ["Space Mono"],
                GenericSlot::Monospace,
            ))
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

/// The body child view (Glyph body family, 12.5, `onSurfaceVariant`).
fn body_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .family(FontFamily::stack_with_generic(
                ["IBM Plex Mono"],
                GenericSlot::Monospace,
            ))
            .size(BODY_SIZE)
            .line_height(LineHeight::Absolute(BODY_LINE_HEIGHT))
            .themed_role(ThemeTextColor::OnSurfaceVariant),
    )
}

/// A state-free "close this dialog" callback — see the [module docs](self)'s
/// "The dismiss callback is state-free" note.
type OnClose = Rc<dyn Fn()>;

/// A declarative Glyph modal dialog. See the [module docs](self).
pub struct GlyphDialogView<State: 'static> {
    title: Option<String>,
    body: Option<String>,
    actions: Vec<AnyView<State>>,
    scrim_dismissible: bool,
    on_close: Option<OnClose>,
}

/// Create an empty Glyph dialog. Chain [`title`](GlyphDialogView::title)/
/// [`body`](GlyphDialogView::body)/[`action`](GlyphDialogView::action) to fill
/// it, and [`on_close`](GlyphDialogView::on_close) to wire dismissal (usually
/// `controller.pop()` — [`show_glyph_dialog`] does this for you).
pub fn glyph_dialog<State: 'static>() -> GlyphDialogView<State> {
    GlyphDialogView {
        title: None,
        body: None,
        actions: Vec::new(),
        scrim_dismissible: true,
        on_close: None,
    }
}

/// PascalCase alias for [`glyph_dialog`], matching the widget-fn vocabulary.
#[allow(non_snake_case)]
pub fn GlyphDialog<State: 'static>() -> GlyphDialogView<State> {
    glyph_dialog()
}

impl<State: 'static> GlyphDialogView<State> {
    /// Set the dialog title (Glyph display family, `onSurface`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the dialog supporting text (Glyph body family, `onSurfaceVariant`).
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Replace the trailing action row (app-provided buttons, in reading order —
    /// the last sits closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Append one action button to the trailing action row.
    pub fn action(mut self, action: AnyView<State>) -> Self {
        self.actions.push(action);
        self
    }

    /// Whether a tap on the scrim (outside the panel) cancels the dialog
    /// (default `true`). Set `false` for a dialog the user must answer via an
    /// action.
    pub fn scrim_dismissible(mut self, dismissible: bool) -> Self {
        self.scrim_dismissible = dismissible;
        self
    }

    /// Set the state-free close callback — invoked once, from paint, when the
    /// exit animation completes after a scrim/Escape cancel. [`show_glyph_dialog`]
    /// wires this to `controller.pop()` automatically.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }
}

/// Push `build`'s dialog as a transparent navigator page (the page below stays
/// visible under the scrim), and register `on_result` for the value the dialog
/// pops with. The scrim/Escape cancel is wired to `controller.pop()` (an empty
/// [`PopResult`]); an action pops with a value via
/// `controller.pop_with_result(..)`.
///
/// The dialog is pushed with [`TransitionSpec::NONE`] — the navigator supplies
/// the modal contract, the widget supplies its own enter/exit staging (see the
/// [module docs](self)).
///
/// ```ignore
/// show_glyph_dialog(
///     &state.nav,
///     || glyph_dialog().title("Delete?").body("This can't be undone.").action(confirm_button),
///     |state: &mut State, result: PopResult| {
///         state.confirmed = result.take::<bool>().unwrap_or(false);
///     },
/// );
/// ```
pub fn show_glyph_dialog<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> GlyphDialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    controller.push_transparent_for_result(
        move || {
            let ctrl = close_ctrl.clone();
            any::<State, _>(build().on_close(move || ctrl.pop()))
        },
        TransitionSpec::NONE,
        on_result,
    );
}

/// The dialog's enter/exit lifecycle phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Animating in (panel `0.94 → 1.0`, scrim fading in).
    Enter,
    /// Fully shown, at rest.
    Shown,
    /// Animating out after a scrim/Escape cancel.
    Exit,
    /// Exit complete; the close callback has fired and the page will be popped.
    Dismissed,
}

/// The retained widget for a [`GlyphDialogView`]. See the [module docs](self).
pub struct GlyphDialogWidget {
    title: Option<ChildPod>,
    title_text: Option<String>,
    body: Option<ChildPod>,
    body_text: Option<String>,
    actions: Vec<ChildPod>,
    scrim_dismissible: bool,
    on_close: Option<OnClose>,
    /// The centered panel rect in the widget's own local coordinate space.
    panel: Rect,
    phase: Phase,
    /// The enter/exit progress driver — lazily built on the first paint that
    /// sees the phase (rebuild has no theme to resolve a [`Timing`] from).
    driver: Option<TransitionDriver>,
    /// A scrim/panel-background press is in flight (the modal barrier captured
    /// the pointer).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside cancels).
    scrim_down_outside: bool,
}

/// Reconcile one optional text child (title/body) in place.
fn reconcile_optional<State: 'static>(
    prev: Option<&String>,
    next: Option<&String>,
    pod: &mut Option<ChildPod>,
    make: impl Fn(&str) -> AnyView<State>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(s)) => {
            *pod = Some(crate::build_child(&make(s.as_str()), ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(s), None) => {
            if let Some(p) = pod.as_mut() {
                crate::teardown_child(&make(s.as_str()), p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => crate::rebuild_child(&make(a.as_str()), &make(b.as_str()), p, ctx),
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

impl<State: 'static> View<State> for GlyphDialogView<State> {
    type Element = GlyphDialogWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphDialogWidget {
        GlyphDialogWidget {
            title: self
                .title
                .as_ref()
                .map(|s| crate::build_child(&title_view::<State>(s), ctx)),
            title_text: self.title.clone(),
            body: self
                .body
                .as_ref()
                .map(|s| crate::build_child(&body_view::<State>(s), ctx)),
            body_text: self.body.clone(),
            actions: self
                .actions
                .iter()
                .map(|v| crate::build_child(v, ctx))
                .collect(),
            scrim_dismissible: self.scrim_dismissible,
            on_close: self.on_close.clone(),
            panel: Rect::ZERO,
            phase: Phase::Enter,
            driver: None,
            scrim_captured: false,
            scrim_down_outside: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphDialogWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        flags |= reconcile_optional(
            prev.title.as_ref(),
            self.title.as_ref(),
            &mut element.title,
            title_view::<State>,
            ctx,
        );
        element.title_text = self.title.clone();
        flags |= reconcile_optional(
            prev.body.as_ref(),
            self.body.as_ref(),
            &mut element.body,
            body_view::<State>,
            ctx,
        );
        element.body_text = self.body.clone();

        let prev_views: Vec<&AnyView<State>> = prev.actions.iter().collect();
        let next_views: Vec<&AnyView<State>> = self.actions.iter().collect();
        flags |= crate::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.actions,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        element.scrim_dismissible = self.scrim_dismissible;
        // Closures aren't comparable; reinstall the close adapter unconditionally.
        element.on_close = self.on_close.clone();
        flags
    }

    fn teardown(&self, element: &mut GlyphDialogWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(s), Some(p)) = (&self.title, element.title.as_mut()) {
            crate::teardown_child(&title_view::<State>(s), p, ctx);
        }
        if let (Some(s), Some(p)) = (&self.body, element.body.as_mut()) {
            crate::teardown_child(&body_view::<State>(s), p, ctx);
        }
        for (view, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

impl GlyphDialogWidget {
    /// Begin the exit animation (a scrim/Escape cancel). Idempotent: only a
    /// `Enter`/`Shown` dialog can start exiting.
    fn begin_exit(&mut self) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.phase = Phase::Exit;
            self.driver = None;
        }
    }

    /// Advance the enter/exit phase machine to frame time `now`, returning the
    /// `(panel_scale, scrim_fraction)` to paint at plus whether the widget is
    /// still animating. Factored out of `paint` so the full timeline is drivable
    /// with synthetic [`FrameTime`]s in a unit test (mirrors
    /// `crate::glyph::toast`'s `advance`).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f32, bool) {
        match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                let p = adv.value;
                let scale = ENTER_SCALE_START + (1.0 - ENTER_SCALE_START) * p;
                (scale, p.clamp(0.0, 1.0) as f32, !adv.done)
            }
            Phase::Shown => (1.0, 1.0, false),
            Phase::Exit => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let q = adv.value.clamp(0.0, 1.0);
                let scale = 1.0 - (1.0 - ENTER_SCALE_START) * q;
                if adv.done {
                    self.phase = Phase::Dismissed;
                    self.driver = None;
                    if let Some(on_close) = &self.on_close {
                        on_close();
                    }
                }
                (scale, (1.0 - q) as f32, !adv.done)
            }
            Phase::Dismissed => (ENTER_SCALE_START, 0.0, false),
        }
    }
}

impl Widget for GlyphDialogWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        let panel_max_w = area_w.min(MAX_WIDTH);
        let content_max_w = (panel_max_w - 2.0 * DIALOG_PADDING).max(0.0);
        let child_bc = BoxConstraints::loose(Size::new(content_max_w, f64::INFINITY));

        let mut content_w: f64 = 0.0;
        let title_size = self.title.as_mut().map(|p| {
            let s = p.layout_child(ctx, &child_bc);
            content_w = content_w.max(s.width);
            s
        });
        let body_size = self.body.as_mut().map(|p| {
            let s = p.layout_child(ctx, &child_bc);
            content_w = content_w.max(s.width);
            s
        });

        let mut actions_w: f64 = 0.0;
        let mut actions_h: f64 = 0.0;
        for (i, pod) in self.actions.iter_mut().enumerate() {
            let s = pod.layout_child(ctx, &child_bc);
            if i > 0 {
                actions_w += ACTION_GAP;
            }
            actions_w += s.width;
            actions_h = actions_h.max(s.height);
        }
        if !self.actions.is_empty() {
            content_w = content_w.max(actions_w);
        }

        let lower = MIN_WIDTH.min(area_w);
        let panel_w = (content_w + 2.0 * DIALOG_PADDING).clamp(lower, panel_max_w);

        let mut y = DIALOG_PADDING;
        let mut title_y = 0.0;
        let mut body_y = 0.0;
        let mut actions_y = 0.0;
        if let Some(s) = title_size {
            title_y = y;
            y += s.height;
        }
        if let Some(s) = body_size {
            if title_size.is_some() {
                y += TITLE_BODY_GAP;
            }
            body_y = y;
            y += s.height;
        }
        if !self.actions.is_empty() {
            if title_size.is_some() || body_size.is_some() {
                y += BODY_ACTIONS_GAP;
            }
            actions_y = y;
            y += actions_h;
        }
        y += DIALOG_PADDING;
        let panel_h = y;

        let panel_x = ((area_w - panel_w) / 2.0).max(0.0);
        let panel_y = ((area_h - panel_h) / 2.0).max(0.0);
        self.panel = Rect::new(panel_x, panel_y, panel_x + panel_w, panel_y + panel_h);

        if let Some(p) = self.title.as_mut() {
            p.set_origin(Point::new(panel_x + DIALOG_PADDING, panel_y + title_y));
        }
        if let Some(p) = self.body.as_mut() {
            p.set_origin(Point::new(panel_x + DIALOG_PADDING, panel_y + body_y));
        }
        let mut ax = panel_x + panel_w - DIALOG_PADDING;
        for pod in self.actions.iter_mut().rev() {
            let w = pod.size().width;
            ax -= w;
            pod.set_origin(Point::new(ax, panel_y + actions_y));
            ax -= ACTION_GAP;
        }

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (enter, exit) = resolve_timings(theme);
        let (scale, scrim_frac, animating) = self.advance(ctx.frame_time(), enter, exit);

        // Scrim fills the whole area (the modal barrier) — its own fade, never
        // scaled with the panel.
        let scrim = with_alpha(resolve_scrim(theme), SCRIM_ALPHA * scrim_frac);
        scene.fill_rect(ctx.origin(), ctx.size(), scrim);

        // The panel scales about its own center; the scrim above did not.
        let panel_origin = Point::new(
            ctx.origin().x + self.panel.x0,
            ctx.origin().y + self.panel.y0,
        );
        let panel_size = Size::new(self.panel.width(), self.panel.height());
        let center = Point::new(
            panel_origin.x + panel_size.width / 2.0,
            panel_origin.y + panel_size.height / 2.0,
        );
        let transform = Affine::translate((center.x, center.y))
            * Affine::scale(scale)
            * Affine::translate((-center.x, -center.y));
        scene.push_transform(transform);

        let (shadow_y, shadow_blur, shadow_color) = resolve_shadow(theme);
        let radius = resolve_radius(theme);
        scene.draw_shadow(
            Point::new(panel_origin.x, panel_origin.y + shadow_y),
            panel_size,
            radius,
            shadow_blur,
            shadow_color,
        );
        scene.fill_rounded_rect(panel_origin, panel_size, radius, resolve_container(theme));
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, panel_size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(
            panel_origin,
            &path,
            BORDER_WIDTH,
            &Brush::Solid(resolve_border(theme)),
        );

        if let Some(p) = self.title.as_mut() {
            p.paint_child(ctx, scene);
        }
        if let Some(p) = self.body.as_mut() {
            p.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
        scene.pop_transform();

        // Keep frames coming while animating, and for one frame past Dismissed
        // so the rebuild that applies the pop actually runs.
        if animating || self.phase == Phase::Dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A press anywhere claims focus so a subsequent Escape has a chain to
        // travel (the material dialog's opt-in).
        if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
            ctx.request_focus();
        }
        // An action already capturing (or freshly hit) consumes the event first.
        if crate::route_event(&mut self.actions, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        // Escape (once focused) begins the exit/cancel.
        if let InputEvent::Key(key_event) = event {
            if key_event.key == Key::Named(NamedKey::Escape) {
                self.begin_exit();
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        // Everything else is the modal barrier.
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.scrim_captured = true;
                self.scrim_down_outside = !self.panel.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.scrim_captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                if !self.scrim_captured {
                    return EventResult::Ignored;
                }
                let released_outside = !self.panel.contains(p.position);
                if self.scrim_dismissible && self.scrim_down_outside && released_outside {
                    self.begin_exit();
                }
                self.scrim_captured = false;
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // Cancel never touches state — just clear the barrier flag.
                self.scrim_captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.title_text.clone();
        ctx.push_container(
            Role::Dialog,
            |node| {
                node.set_modal();
                if let Some(label) = &label {
                    node.set_label(label.as_str());
                }
            },
            |ctx| {
                if let Some(p) = &self.title {
                    p.semantics_child(ctx);
                }
                if let Some(p) = &self.body {
                    p.semantics_child(ctx);
                }
                for pod in &self.actions {
                    pod.semantics_child(ctx);
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
        BuildCtx, KeyEvent, Modifiers, PointerButton, PointerEvent, RenderRoot, any as core_any,
    };
    use frust_text::TextContext;
    use std::any::Any;
    use std::cell::Cell;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn build<S: 'static>(view: &GlyphDialogView<S>) -> GlyphDialogWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records fills, rounded fills, shadows, strokes, and transform push/pops.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: usize,
        strokes: Vec<Color>,
        transforms: Vec<Affine>,
        transform_pops: u32,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {
            self.shadows += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    // -- Enter/exit staging (advance table) -----------------------------

    #[test]
    fn enter_staging_scales_panel_094_to_1_and_fades_scrim_in() {
        let view: GlyphDialogView<()> = glyph_dialog().title("Hi");
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        // Seed the clock: panel starts at 0.94, scrim invisible.
        let (scale0, scrim0, animating0) = w.advance(ft_ms(0.0), enter, exit);
        assert!((scale0 - ENTER_SCALE_START).abs() < 1e-6);
        assert!(scrim0.abs() < 1e-6);
        assert!(animating0);
        assert_eq!(w.phase, Phase::Enter);

        // Past the 220ms enter: settles fully shown (scale 1.0, scrim 1.0).
        let (scale1, scrim1, animating1) = w.advance(ft_ms(400.0), enter, exit);
        assert!((scale1 - 1.0).abs() < 1e-6);
        assert!((scrim1 - 1.0).abs() < 1e-6);
        assert!(!animating1);
        assert_eq!(w.phase, Phase::Shown);
    }

    #[test]
    fn exit_is_faster_than_enter_and_reverses_the_staging() {
        // The exit driver (150ms) settles well before the enter driver (220ms).
        let view: GlyphDialogView<()> = glyph_dialog().title("Hi");
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit); // -> Shown
        w.begin_exit();
        assert_eq!(w.phase, Phase::Exit);

        // Seed the exit clock: starts fully shown.
        let (scale_seed, scrim_seed, _) = w.advance(ft_ms(400.0), enter, exit);
        assert!((scale_seed - 1.0).abs() < 1e-6);
        assert!((scrim_seed - 1.0).abs() < 1e-6);

        // Still exiting 149ms in (would already be done at the 150ms exit, but
        // not yet at the 220ms enter — proving the exit is the faster driver).
        let (_, _, animating_mid) = w.advance(ft_ms(400.0 + 149.0), enter, exit);
        assert!(animating_mid);

        // Past the 150ms exit: collapsed and Dismissed.
        let (scale_end, scrim_end, animating_end) = w.advance(ft_ms(400.0 + 200.0), enter, exit);
        assert!((scale_end - ENTER_SCALE_START).abs() < 1e-6);
        assert!(scrim_end.abs() < 1e-6);
        assert!(!animating_end);
        assert_eq!(w.phase, Phase::Dismissed);
    }

    #[test]
    fn on_close_fires_once_when_the_exit_completes() {
        let count = Rc::new(Cell::new(0u32));
        let c = count.clone();
        let view: GlyphDialogView<()> = glyph_dialog()
            .title("Hi")
            .on_close(move || c.set(c.get() + 1));
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit);
        w.begin_exit();
        w.advance(ft_ms(400.0), enter, exit);
        assert_eq!(count.get(), 0, "on_close waits for the exit to finish");
        w.advance(ft_ms(700.0), enter, exit); // completes the exit
        assert_eq!(count.get(), 1);
        // Idempotent past Dismissed.
        w.advance(ft_ms(900.0), enter, exit);
        assert_eq!(count.get(), 1);
    }

    #[test]
    fn scrim_uses_own_fade_while_panel_scales_under_a_transform() {
        // Paint at enter start: the scrim fill is emitted *before* the transform
        // push (its own fade), and the panel fill/border ride inside the
        // transform — so scrim and panel stage independently.
        let view: GlyphDialogView<()> = glyph_dialog().title("Hi");
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        // A full-area scrim fill, at (near) zero alpha at the seeded start.
        assert_eq!(rec.rects[0].1, area);
        assert!(
            rec.rects[0].2.components[3] < 1e-3,
            "scrim starts transparent"
        );
        // Panel fill + border stroke + shadow emitted under one transform.
        assert_eq!(rec.transforms.len(), 1);
        assert_eq!(rec.transform_pops, 1);
        assert_eq!(rec.rrects.len(), 1, "the panel surface fill");
        assert_eq!(rec.strokes.len(), 1, "the panel border stroke");
        assert_eq!(rec.shadows, 1, "the chrome-level shadow");
        assert!(
            pctx.needs_frame(),
            "an in-flight enter requests another frame"
        );
    }

    // -- Token assertions (dark + light) --------------------------------

    #[test]
    fn glyph_dark_and_light_resolve_panel_ring_and_scrim() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
        for theme in [&dark, &light] {
            assert_eq!(
                resolve_container(Some(theme)),
                theme.scheme().surface_container_high
            );
            assert_eq!(resolve_border(Some(theme)), theme.scheme().outline);
            assert_eq!(resolve_scrim(Some(theme)), theme.scheme().scrim);
            assert_eq!(resolve_radius(Some(theme)), theme.shape.large);
        }
        // Dark and light panels differ (not a mere value flip).
        assert_ne!(
            resolve_container(Some(&dark)),
            resolve_container(Some(&light))
        );
    }

    #[test]
    fn themed_paint_fills_panel_with_glyph_surface() {
        let theme = Theme::glyph_baseline();
        let view: GlyphDialogView<()> = glyph_dialog().title("Confirm");
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let area = Size::new(400.0, 600.0);
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(&theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        // Advance to Shown so the panel is at full opacity/scale.
        let mut pctx = PaintCtx::new(Point::ZERO, area).with_theme(&theme);
        w.phase = Phase::Shown;
        w.paint(&mut pctx, &mut rec);
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_high);
        assert_eq!(rec.rrects[0].2, theme.shape.large);
        assert_eq!(rec.strokes[0], theme.scheme().outline);
    }

    // -- Navigator integration: scrim cancel pops with an empty result --

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<bool>>,
    }

    fn app_page(
        controller: &NavigatorController<NavState>,
    ) -> impl FnMut(&mut NavState) -> NavigatorView<NavState> {
        let ctrl = controller.clone();
        move |_: &mut NavState| navigator(&ctrl, || core_any::<NavState, _>(sized(400.0, 600.0)))
    }

    struct Sized {
        size: Size,
    }
    struct SizedW {
        size: Size,
    }
    impl View<NavState> for Sized {
        type Element = SizedW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> SizedW {
            SizedW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut SizedW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for SizedW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn sized(w: f64, h: f64) -> Sized {
        Sized {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn scrim_cancel_animates_then_pops_dialog_with_empty_result() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_glyph_dialog(
            &controller,
            || glyph_dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        // Settle the enter animation.
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // Tap the scrim (top-left, outside the centered panel).
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert!(
            state.results.is_empty(),
            "the exit animation has not finished yet"
        );

        // Drive the exit to completion: on_close (controller.pop()) fires, then
        // the next rebuild applies the pop and the result callback flushes.
        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim cancel pops with an empty result"
        );
    }

    #[test]
    fn non_dismissible_scrim_does_not_cancel() {
        let view: GlyphDialogView<()> = glyph_dialog().title("Hi").scrim_dismissible(false);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));

        let mut dummy = ();
        let dispatch = |w: &mut GlyphDialogWidget, dummy: &mut (), e: &InputEvent| {
            let s: &mut dyn Any = dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, Size::new(400.0, 600.0));
            w.event(&mut ctx, e)
        };
        dispatch(&mut w, &mut dummy, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut dummy, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(
            w.phase,
            Phase::Enter,
            "a non-dismissible scrim never begins exit"
        );
    }

    #[test]
    fn escape_begins_exit_after_a_press_claims_focus() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_glyph_dialog(
            &controller,
            || glyph_dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<bool>()),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // A press claims focus; Escape then begins the exit and eventually pops.
        let (cx, cy) = (area.width / 2.0, area.height / 2.0);
        root.event(&mut state, &ev(PointerPhase::Down, cx, cy));
        root.event(&mut state, &ev(PointerPhase::Up, cx, cy));
        root.event(&mut state, &escape_event());
        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "Escape dismisses the focused dialog"
        );
    }

    #[test]
    fn action_pops_with_a_value_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        let action_w = 80.0;
        let action_h = 36.0;
        {
            let confirm = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let c = confirm.clone();
                    core_any::<NavState, _>(glyph_dialog().action(tap_action(
                        action_w,
                        action_h,
                        move |_s: &mut NavState| c.pop_with_result(PopResult::of(true)),
                    )))
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| state.results.push(result.take::<bool>()),
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // With no title/body the panel is MIN_WIDTH wide, centered; the action
        // hugs the trailing edge, DIALOG_PADDING from the top.
        let panel_x = (area.width - MIN_WIDTH) / 2.0;
        let panel_h = 2.0 * DIALOG_PADDING + action_h;
        let panel_y = (area.height - panel_h) / 2.0;
        let action_cx = panel_x + MIN_WIDTH - DIALOG_PADDING - action_w / 2.0;
        let action_cy = panel_y + DIALOG_PADDING + action_h / 2.0;

        root.event(&mut state, &ev(PointerPhase::Down, action_cx, action_cy));
        root.event(&mut state, &ev(PointerPhase::Up, action_cx, action_cy));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(state.results, vec![Some(true)], "the action pops with true");
    }

    // -- Suppressing-container contract: no dangling capture after dismiss --

    #[test]
    fn no_dangling_capture_after_dismiss() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_glyph_dialog(
            &controller,
            || glyph_dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<bool>()),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        // Press+release on the scrim: the modal barrier captured the pointer and
        // begins the exit. Drive it to completion and pop.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        root.paint(&mut Recorder::default(), ft_ms(500.0));
        root.paint(&mut Recorder::default(), ft_ms(800.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // The dialog page is gone; the underlying page now receives events. A
        // stray Up must not be routed into a torn-down dialog's captured barrier.
        assert!(
            !root.is_pointer_captured(),
            "no capture survives the dismiss"
        );
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.results, vec![None]);
    }

    #[test]
    fn semantics_is_a_modal_dialog_labelled_by_title() {
        fn logic(_s: &mut ()) -> GlyphDialogView<()> {
            glyph_dialog().title("Confirm").body("Sure?")
        }
        let mut root: RenderRoot<(), GlyphDialogView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the dialog node sets the modal flag");
        assert_eq!(node.label(), Some("Confirm"));
        assert!(
            !node.children().is_empty(),
            "title/body are semantics children"
        );
    }

    // A fixed-size interactive leaf firing `on_tap` on up-inside (a dialog action
    // of known geometry, no text shaping) — mirrors the material dialog fixture.
    struct TapView<State: 'static> {
        size: Size,
        on_tap: Rc<dyn Fn(&mut State)>,
    }
    struct TapWidget {
        size: Size,
        on_tap: crate::ErasedCallback,
        captured: bool,
    }
    impl<State: 'static> View<State> for TapView<State> {
        type Element = TapWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TapWidget {
            TapWidget {
                size: self.size,
                on_tap: crate::erase_callback(&self.on_tap),
                captured: false,
            }
        }
        fn rebuild(&self, _p: &Self, e: &mut TapWidget, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            e.on_tap = crate::erase_callback(&self.on_tap);
            ChangeFlags::NONE
        }
    }
    impl Widget for TapWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Ignored;
            };
            match p.phase {
                PointerPhase::Down => {
                    self.captured = true;
                    ctx.capture_pointer();
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.captured
                        && p.position.x >= 0.0
                        && p.position.y >= 0.0
                        && p.position.x < self.size.width
                        && p.position.y < self.size.height
                    {
                        (self.on_tap)(ctx);
                    }
                    self.captured = false;
                    EventResult::Handled
                }
                PointerPhase::Move => EventResult::Handled,
                PointerPhase::Cancel => {
                    self.captured = false;
                    EventResult::Handled
                }
            }
        }
    }
    fn tap_action<State: 'static>(
        w: f64,
        h: f64,
        on_tap: impl Fn(&mut State) + 'static,
    ) -> AnyView<State> {
        core_any::<State, _>(TapView {
            size: Size::new(w, h),
            on_tap: Rc::new(on_tap),
        })
    }
}
