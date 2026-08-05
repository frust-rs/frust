//! The basic M3 `Dialog`: a full-area widget = **scrim** (32% scrim-color
//! fill; tap outside the panel dismisses) + a **centered panel** (extra-large
//! 28dp shape, `surfaceContainerHigh`, 24dp padding all around), pushed as a
//! transparent navigator page via
//! [`NavigatorController::push_transparent_for_result`] and entering via the
//! existing [`PageTransition::M3FadeThrough`] preset.
//!
//! # Navigator-modal architecture
//!
//! The navigator provides **no scrim** and routes input only to the top page
//! (verified) — so the scrim is part of *this* widget, not the navigator. A
//! dialog is designed to be pushed as a transparent page (the page below stays
//! visible under the scrim); [`show_dialog`] is the convenience that wraps the
//! push. Dismissal is always `controller.pop()`:
//!
//! * a **scrim tap** (a press+release outside the panel) pops with an *empty*
//!   [`PopResult`] — a plain back-navigation;
//! * an **action** button (an app-provided child in the trailing action row)
//!   pops with a value via `controller.pop_with_result(PopResult::of(..))`,
//!   delivered to the pusher's `on_result` callback by the navigator's existing
//!   pop-result machinery.
//!
//! The panel itself swallows every pointer event that no action consumes (a
//! modal barrier), so a tap on the panel background neither dismisses nor leaks
//! through to whatever is below.
//!
//! # Panel anatomy
//!
//! 24dp padding on all four edges; `title` in the M3 `headlineSmallEmphasized`
//! type role (`onSurface`; the emphasized-type consumption — same
//! size/line-height as `headlineSmall`, weight stepped to Medium), `body` in
//! `bodyMedium` (`onSurfaceVariant`), and a trailing,
//! right-aligned row of app-provided action buttons. Internal spacing follows
//! the M3 basic-dialog spec: 16dp title→body, 24dp body→actions, 8dp between
//! buttons. The panel width is clamped to `[280, 560]`dp (M3 min/max), the
//! panel is centered in the available area, and it sizes to its content.
//!
//! # Standalone use
//!
//! A `Dialog` is a plain widget: it works inside a [`crate::Stack`] too (the
//! scrim is still its own — `Stack` hit-tests topmost-first, so a dialog on top
//! of a stack is modal over the children below it). The navigator path is the
//! primary, documented one (Flutter precedent: dialogs are routes). This module
//! does **not** edit any nav file; the modal API it relies on
//! ([`push_transparent_for_result`](NavigatorController::push_transparent_for_result))
//! is consumed read-only here.
//!
//! # Semantics
//!
//! The whole dialog contributes one [`Role::Dialog`] container node with the
//! accesskit **modal** flag set (per the accesskit vocabulary for a modal
//! dialog box), labelled with the title text; the title, body, and each action
//! become its accesskit children.
//!
//! # Keyboard operability
//!
//! **Escape-to-dismiss now works, once the dialog has focus.** A `Down`
//! anywhere in the dialog (scrim, panel background, or an action) claims
//! focus via `EventCtx::request_focus` — the dialog already captures its
//! whole area, so this is a pure opt-in with no new hit-testing. Once
//! focused, a focus-routed `Key(Escape)` invokes the *same* dismiss path as a
//! scrim tap (`on_dismiss`). **There is still no hook to focus the dialog on
//! appear** (auto-focus-on-appear) — a caller must complete one pointer
//! interaction with the dialog before Escape does anything; that gap is
//! deferred to a future focus-manager work item.
//!
//! The accesskit **modal** flag set above is nonetheless **kept deliberately**,
//! not dropped. No platform `accesskit_*` adapter is wired yet (see
//! `docs/ARCHITECTURE.md`'s Semantics pass), so there is no live assistive-tech
//! audience the flag could currently mislead.
//!
//! # State layer
//!
//! This dialog has no `StateLayer` surface of its own (the scrim and panel are
//! plain fills, not an M3 interactive surface) — there is nothing here for
//! `StateLayer::set_focused` to wire into. Only widgets with their own state
//! layer (none in this module) would gain focused-state chrome; this dialog
//! does not invent one.

use std::rc::Rc;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_text::{FontWeight, LineHeight};
use frust_theme::Theme;
use kurbo::{Point, Rect, Size};
use peniko::Color;

use crate::authoring::ThemeTextColor;
use crate::nav::navigator::{NavigatorController, PopResult};
use crate::nav::transition::{PageTransition, TransitionSpec};
use crate::text::text;

/// Scrim opacity behind a modal dialog (M3 spec: 32%).
const SCRIM_ALPHA: f32 = 0.32;
/// Panel padding on all four edges, in logical px (M3 basic-dialog spec).
const DIALOG_PADDING: f64 = 24.0;
/// Vertical spacing between the title and body (M3 basic-dialog spec).
const TITLE_BODY_GAP: f64 = 16.0;
/// Vertical spacing between the body and the action row (M3 spec).
const BODY_ACTIONS_GAP: f64 = 24.0;
/// Horizontal spacing between adjacent action buttons (M3 spec).
const ACTION_GAP: f64 = 8.0;
/// Minimum dialog panel width, in logical px (M3 dialog spec).
const MIN_WIDTH: f64 = 280.0;
/// Maximum dialog panel width, in logical px (M3 dialog spec).
const MAX_WIDTH: f64 = 560.0;
/// Unthemed-fallback panel corner radius (a theme resolves this from
/// `shape.extra_large`, a 28dp token).
const RADIUS: f64 = 28.0;

/// Title type-scale token (M3 `headlineSmallEmphasized` — matches
/// `frust-theme::typography`'s `HEADLINE_SMALL_EMPHASIZED`: same size 24 /
/// line-height 32 as the baseline `HEADLINE_SMALL`, weight stepped up to
/// Medium). Hardcoded rather than read from a live `Theme::type_scale`:
/// `Text` has no layout-time-deferred *size/weight* resolution seam (only a
/// *color* role can be resolved after `View::build` — see
/// `docs/CODE_STANDARDS.md`'s Theming conventions), the same precedent
/// [`crate::material::appbar`] follows. The emphasized weight is stepped up
/// from the default `Text` weight (`REGULAR`), which matches the baseline
/// `headlineSmall` token.
const TITLE_SIZE: f32 = 24.0;
const TITLE_LINE_HEIGHT: f32 = 32.0;
const TITLE_WEIGHT: FontWeight = FontWeight::MEDIUM;
/// Body type-scale token (M3 `bodyMedium` — size 14 / line-height 20).
const BODY_SIZE: f32 = 14.0;
const BODY_LINE_HEIGHT: f32 = 20.0;

/// Unthemed-fallback panel container fill (a theme resolves this from
/// `colors.surface_container_high`).
const CONTAINER: Color = Color::from_rgb8(0xEC, 0xE6, 0xF0);
/// Unthemed-fallback scrim base color (a theme resolves this from
/// `colors.scrim`); applied at [`SCRIM_ALPHA`].
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`crate::material::card`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved scrim fill. Themed: `colors.scrim` at [`SCRIM_ALPHA`]. Unthemed:
/// [`SCRIM`] at [`SCRIM_ALPHA`].
fn resolve_scrim(theme: Option<&Theme>) -> Color {
    let base = match theme {
        Some(theme) => theme.scheme().scrim,
        None => SCRIM,
    };
    with_alpha(base, SCRIM_ALPHA)
}

/// The resolved panel container fill. Themed: `colors.surface_container_high`.
/// Unthemed: [`CONTAINER`] exactly.
fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().surface_container_high,
        None => CONTAINER,
    }
}

/// The resolved panel corner radius. Themed: `shape.extra_large`. Unthemed:
/// [`RADIUS`] exactly.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.extra_large,
        None => RADIUS,
    }
}

/// Coerce a possibly-infinite constraint dimension to a finite value (a dialog
/// expects bounded constraints — a navigator page or a full-screen `Stack`;
/// mirrors [`crate::material::appbar`]'s finite-width guard).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// Build the title's type-erased child view (`headlineSmallEmphasized`,
/// themed `onSurface` — the `Text` default role).
fn title_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

/// Build the body's type-erased child view (`bodyMedium`, themed
/// `onSurfaceVariant` — the M3 supporting-text role, R15).
fn body_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(BODY_SIZE)
            .line_height(LineHeight::Absolute(BODY_LINE_HEIGHT))
            .themed_role(ThemeTextColor::OnSurfaceVariant),
    )
}

/// A view-held, typed scrim-dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A declarative basic M3 dialog. See the [module docs](self).
pub struct DialogView<State: 'static> {
    title: Option<String>,
    body: Option<String>,
    actions: Vec<AnyView<State>>,
    on_dismiss: Option<OnDismiss<State>>,
}

/// Create an empty dialog. Chain [`DialogView::title`]/[`DialogView::body`]/
/// [`DialogView::action`]/[`DialogView::actions`] to fill it, and
/// [`DialogView::on_dismiss`] to handle a scrim tap (usually a
/// `controller.pop()` — [`show_dialog`] wires this for you).
pub fn dialog<State: 'static>() -> DialogView<State> {
    DialogView {
        title: None,
        body: None,
        actions: Vec::new(),
        on_dismiss: None,
    }
}

impl<State: 'static> DialogView<State> {
    /// Set the dialog title (`headlineSmallEmphasized`, `onSurface`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the dialog supporting text (`bodyMedium`, `onSurfaceVariant`).
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Replace the trailing action row with `actions` (app-provided buttons, in
    /// reading order — the last one sits closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Append one action button to the trailing action row.
    pub fn action(mut self, action: AnyView<State>) -> Self {
        self.actions.push(action);
        self
    }

    /// Set the scrim-tap dismiss callback — invoked with `&mut State` when the
    /// user taps outside the panel. [`show_dialog`] wires this to
    /// `controller.pop()` automatically.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }
}

/// Push `build`'s dialog as a transparent navigator page (the page below stays
/// visible under the scrim), entering via [`PageTransition::M3FadeThrough`], and
/// register `on_result` for the value the dialog pops with.
///
/// The dialog's scrim-tap dismissal is wired to `controller.pop()` for you (a
/// scrim tap pops with an *empty* [`PopResult`], overriding any
/// [`DialogView::on_dismiss`] the builder set); an action button pops with a
/// value via `controller.pop_with_result(..)`.
///
/// ```ignore
/// show_dialog(
///     &state.nav,
///     || dialog().title("Delete?").body("This can't be undone.").action(confirm_button),
///     |state: &mut State, result: PopResult| {
///         state.confirmed = result.take::<bool>().unwrap_or(false);
///     },
/// );
/// ```
pub fn show_dialog<State, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    B: Fn() -> DialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let dismiss_ctrl = controller.clone();
    controller.push_transparent_for_result(
        move || {
            let ctrl = dismiss_ctrl.clone();
            any::<State, _>(build().on_dismiss(move |_state: &mut State| ctrl.pop()))
        },
        TransitionSpec::duration(PageTransition::M3FadeThrough),
        on_result,
    );
}

/// The retained widget for a [`DialogView`]. See the [module docs](self).
pub struct DialogWidget {
    title: Option<ChildPod>,
    title_text: Option<String>,
    body: Option<ChildPod>,
    body_text: Option<String>,
    actions: Vec<ChildPod>,
    on_dismiss: Option<crate::authoring::ErasedCallback>,
    /// The centered panel rect in the widget's own local coordinate space
    /// (computed at layout, read for scrim/panel hit-testing at event time).
    panel: Rect,
    /// A scrim/panel-background press is in flight (the modal barrier captured
    /// the pointer). Set on a `Down` no action consumed.
    scrim_captured: bool,
    /// Whether that press started *outside* the panel — only an outside-press
    /// released outside dismisses (a press on the panel background is swallowed
    /// but never dismisses).
    scrim_down_outside: bool,
    /// Whether a `Down` has already claimed focus this open — the claim-once
    /// guard (see `event`'s Down arm). Seeding `false` in [`View::build`] is
    /// correct with no reset needed elsewhere: [`show_dialog`] pushes a fresh
    /// page per open, so a new widget instance (and a fresh `false`) is built
    /// every time the dialog opens; there is no retained instance to reset
    /// on close.
    focus_claimed: bool,
}

/// Reconcile one optional text child (title/body) in place: build it when it
/// appears, tear it down when it goes away, rebuild it when it changes.
fn reconcile_optional<State: 'static>(
    prev: Option<&String>,
    next: Option<&String>,
    pod: &mut Option<ChildPod>,
    make: impl Fn(&str) -> AnyView<State>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(s)) => {
            *pod = Some(crate::authoring::build_child(&make(s.as_str()), ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(s), None) => {
            if let Some(p) = pod.as_mut() {
                crate::authoring::teardown_child(&make(s.as_str()), p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => {
                crate::authoring::rebuild_child(&make(a.as_str()), &make(b.as_str()), p, ctx)
            }
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

impl<State: 'static> View<State> for DialogView<State> {
    type Element = DialogWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DialogWidget {
        DialogWidget {
            title: self
                .title
                .as_ref()
                .map(|s| crate::authoring::build_child(&title_view::<State>(s), ctx)),
            title_text: self.title.clone(),
            body: self
                .body
                .as_ref()
                .map(|s| crate::authoring::build_child(&body_view::<State>(s), ctx)),
            body_text: self.body.clone(),
            actions: self
                .actions
                .iter()
                .map(|v| crate::authoring::build_child(v, ctx))
                .collect(),
            on_dismiss: self
                .on_dismiss
                .as_ref()
                .map(crate::authoring::erase_callback),
            panel: Rect::ZERO,
            scrim_captured: false,
            scrim_down_outside: false,
            focus_claimed: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DialogWidget,
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
        flags |= crate::authoring::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.actions,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        // Closures aren't comparable, so reinstall the dismiss adapter
        // unconditionally (cheap — mirrors every other interactive widget).
        element.on_dismiss = self
            .on_dismiss
            .as_ref()
            .map(crate::authoring::erase_callback);
        flags
    }

    fn teardown(&self, element: &mut DialogWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(s), Some(p)) = (&self.title, element.title.as_mut()) {
            crate::authoring::teardown_child(&title_view::<State>(s), p, ctx);
        }
        if let (Some(s), Some(p)) = (&self.body, element.body.as_mut()) {
            crate::authoring::teardown_child(&body_view::<State>(s), p, ctx);
        }
        for (view, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            crate::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for DialogWidget {
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

        // Measure each action at its natural size; accumulate the row width.
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

        // Vertical stack: title, body, action row — each present only if set.
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
        // Right-align the action row: the last action hugs the trailing edge.
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
        // Scrim fills the whole widget area (the modal barrier over the page).
        scene.fill_rect(ctx.origin(), ctx.size(), resolve_scrim(theme));

        let container = resolve_container(theme);
        let radius = resolve_radius(theme);
        let panel_origin = Point::new(
            ctx.origin().x + self.panel.x0,
            ctx.origin().y + self.panel.y0,
        );
        let panel_size = Size::new(self.panel.width(), self.panel.height());
        scene.fill_rounded_rect(panel_origin, panel_size, radius, container);

        if let Some(p) = self.title.as_mut() {
            p.paint_child(ctx, scene);
        }
        if let Some(p) = self.body.as_mut() {
            p.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The first press in the modal claims focus, so a subsequent Escape
        // has a focus chain to travel (see the module docs' Keyboard
        // operability note — this is the opt-in that note describes); the
        // claim is held until this page pops, so a later press re-claiming
        // would be redundant.
        if !self.focus_claimed
            && matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down)
        {
            ctx.request_focus();
            self.focus_claimed = true;
        }
        // An action already capturing (or freshly hit) consumes the event first.
        if crate::authoring::route_event(&mut self.actions, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        // Escape (once focused) dismisses through the same path as a scrim tap.
        if let InputEvent::Key(key_event) = event {
            if key_event.key == Key::Named(NamedKey::Escape)
                && let Some(on_dismiss) = self.on_dismiss.as_mut()
            {
                on_dismiss(ctx);
                return EventResult::Handled;
            }
            return EventResult::Ignored;
        }
        // Everything else is the modal barrier: pointer events are swallowed, a
        // press+release outside the panel dismisses. Non-pointer events the
        // actions ignored fall through (the dialog owns no scroll target).
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
                if self.scrim_down_outside
                    && released_outside
                    && let Some(on_dismiss) = self.on_dismiss.as_mut()
                {
                    on_dismiss(ctx);
                }
                self.scrim_captured = false;
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // A Cancel arm never touches state (the `()` Cancel tripwire
                // contract) — just clear the barrier flag.
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
    use crate::nav::transition::TransitionSpec;
    use frust_core::{
        FrameTime, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, RenderRoot,
        any as core_any,
    };
    use frust_text::TextContext;

    fn escape_event() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    // --- A fixed-size interactive leaf: fires `on_tap` on up-inside. Lets a
    //     test place a dialog action of known geometry without text shaping. ---

    struct TapView<State: 'static> {
        size: Size,
        on_tap: Rc<dyn Fn(&mut State)>,
    }
    struct TapWidget {
        size: Size,
        on_tap: crate::authoring::ErasedCallback,
        captured: bool,
    }
    impl<State: 'static> View<State> for TapView<State> {
        type Element = TapWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TapWidget {
            TapWidget {
                size: self.size,
                on_tap: crate::authoring::erase_callback(&self.on_tap),
                captured: false,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut TapWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.on_tap = crate::authoring::erase_callback(&self.on_tap);
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

    fn build<S: 'static>(view: &DialogView<S>) -> DialogWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records filled rects and rounded rects (origin, size, radius, color).
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    #[test]
    fn layout_centers_panel_and_stacks_children() {
        let view: DialogView<()> =
            dialog()
                .title("Title")
                .body("Body")
                .action(tap_action(80.0, 36.0, |_s: &mut ()| {}));
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let area = Size::new(400.0, 600.0);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(area));
        assert_eq!(size, area, "dialog fills the whole area (scrim)");

        // The panel is horizontally centered and at least MIN_WIDTH wide.
        assert!(w.panel.width() >= MIN_WIDTH);
        let left_gap = w.panel.x0;
        let right_gap = area.width - w.panel.x1;
        assert!((left_gap - right_gap).abs() < 1e-6, "panel is centered");

        // Title sits above the body, both inset by the padding.
        let title = w.title.as_ref().unwrap();
        let body = w.body.as_ref().unwrap();
        assert_eq!(title.origin().x, w.panel.x0 + DIALOG_PADDING);
        assert!(body.origin().y > title.origin().y);

        // The single action hugs the panel's trailing edge.
        let action = &w.actions[0];
        assert_eq!(
            action.origin().x + action.size().width,
            w.panel.x1 - DIALOG_PADDING
        );
    }

    #[test]
    fn unthemed_paint_draws_scrim_then_panel() {
        let view: DialogView<()> = dialog();
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);

        // Scrim: a full-area fill at 32% of the fallback scrim color.
        assert_eq!(rec.rects[0].1, area);
        assert_eq!(rec.rects[0].2, with_alpha(SCRIM, SCRIM_ALPHA));
        // Panel: fallback container fill at the fallback (28dp) radius.
        assert_eq!(rec.rrects[0].2, RADIUS);
        assert_eq!(rec.rrects[0].3, CONTAINER);
    }

    #[test]
    fn themed_paint_resolves_r15_tokens() {
        let theme = Theme::m3_baseline();
        let scheme = theme.scheme();
        let view: DialogView<()> = dialog();
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let area = Size::new(400.0, 600.0);
        w.layout(&mut lctx, &BoxConstraints::tight(area));

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rects[0].2, with_alpha(scheme.scrim, SCRIM_ALPHA));
        assert_eq!(rec.rrects[0].2, theme.shape.extra_large);
        assert_eq!(rec.rrects[0].3, scheme.surface_container_high);
    }

    // --- Standalone (no navigator) scrim-dismiss behavior. ---

    #[derive(Default)]
    struct Flag {
        dismissed: u32,
    }

    fn dispatch<S: 'static>(
        w: &mut DialogWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, 600.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn scrim_press_release_outside_panel_fires_on_dismiss() {
        let view: DialogView<Flag> = dialog()
            .title("Hi")
            .on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));

        let mut state = Flag::default();
        // (5, 5) is in the top-left scrim, well outside the centered panel.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn press_on_panel_background_is_swallowed_without_dismissing() {
        let view: DialogView<Flag> = dialog()
            .title("Hi")
            .on_dismiss(|s: &mut Flag| s.dismissed += 1);
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(400.0, 600.0)));

        let mut state = Flag::default();
        let center = w.panel.center();
        let r = dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, center.x, center.y),
        );
        assert_eq!(
            r,
            EventResult::Handled,
            "the modal barrier swallows the press"
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, center.x, center.y),
        );
        assert_eq!(state.dismissed, 0, "a tap on the panel does not dismiss");
    }

    // --- Navigator integration: scrim tap pops with an empty result. ---

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

    // A minimal opaque background page.
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
    fn scrim_tap_pops_dialog_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // Push a dialog directly with an instant transition (no fade to settle),
        // wiring the scrim dismiss to a plain pop — exactly show_dialog's shape.
        {
            let dismiss = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let d = dismiss.clone();
                    core_any::<NavState, _>(
                        dialog()
                            .title("Confirm")
                            .on_dismiss(move |_s: &mut NavState| d.pop()),
                    )
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<bool>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // Tap the scrim (top-left corner, outside the centered panel).
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        // The dismiss pop is applied at rebuild; the result callback flushes on
        // the following event pass.
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim tap pops with an empty result"
        );
    }

    // --- Focus + Escape opt-in. ---

    #[test]
    fn escape_after_a_press_claims_focus_and_dismisses_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_dialog(
            &controller,
            dialog,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        // Settle the entering M3FadeThrough transition so input reaches the page.
        root.paint(&mut Recorder::default(), FrameTime::ZERO);
        root.paint(
            &mut Recorder::default(),
            FrameTime::from_nanos(1_000_000_000),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // A press+release on the panel background (an empty dialog's panel is
        // MIN_WIDTH x 2*DIALOG_PADDING, centered) claims focus but is swallowed —
        // it does not itself dismiss (mirrors
        // `press_on_panel_background_is_swallowed_without_dismissing`).
        let (cx, cy) = (area.width / 2.0, area.height / 2.0);
        root.event(&mut state, &ev(PointerPhase::Down, cx, cy));
        root.event(&mut state, &ev(PointerPhase::Up, cx, cy));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        assert!(
            state.results.is_empty(),
            "a panel-background press does not dismiss"
        );

        // Escape now reaches the focused dialog and dismisses it, the same
        // dismiss path as a scrim tap.
        root.event(&mut state, &escape_event());
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
    fn second_press_does_not_reclaim_focus_after_one_open() {
        use frust_core::ChildPod;

        let view: DialogView<()> = dialog().title("Hi");
        let area = Size::new(400.0, 600.0);
        let w = build(&view);
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area));

        let mut dummy = ();
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, 5.0, 5.0));
        }

        // Simulate an external clear so the second Down's own effect on the
        // recorded flag is isolated: if the guard holds, the widget itself
        // never re-calls `request_focus`, so the flag stays as we set it.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area);
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 5.0, 5.0));
        }
        assert!(
            !pod.is_focused(),
            "a second Down within the same open must not re-request focus"
        );
    }

    #[test]
    fn escape_without_a_prior_press_does_nothing() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_dialog(
            &controller,
            dialog,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), FrameTime::ZERO);
        root.paint(
            &mut Recorder::default(),
            FrameTime::from_nanos(1_000_000_000),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        // No pointer press yet — the dialog never claimed focus, so Escape has
        // no focus chain to travel and is dropped (route_event_single finds no
        // focused page pod).
        root.event(&mut state, &escape_event());
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert!(
            state.results.is_empty(),
            "Escape without prior focus is a no-op"
        );
    }

    #[test]
    fn action_pops_with_a_value_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // A dialog with a single fixed-size action that pops with `true`.
        let action_w = 80.0;
        let action_h = 36.0;
        {
            let confirm = controller.clone();
            controller.push_transparent_for_result(
                move || {
                    let c = confirm.clone();
                    core_any::<NavState, _>(dialog().action(tap_action(
                        action_w,
                        action_h,
                        move |_s: &mut NavState| c.pop_with_result(PopResult::of(true)),
                    )))
                },
                TransitionSpec::NONE,
                |state: &mut NavState, result: PopResult| {
                    state.results.push(result.take::<bool>());
                },
            );
        }
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(Size::new(400.0, 600.0), &mut tcx as &mut dyn Any);

        // With no title/body, the panel is MIN_WIDTH wide, centered; the action
        // hugs the trailing edge, DIALOG_PADDING from the top.
        let area = Size::new(400.0, 600.0);
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

    #[test]
    fn show_dialog_pushes_transparent_dialog_over_the_page() {
        // Drives the real show_dialog path (M3FadeThrough): after the fade
        // settles, the dialog scrim is painted over the still-visible page.
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(400.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_dialog(
            &controller,
            || dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        // Advance the fade to completion, then finalize it via one more rebuild.
        root.paint(&mut Recorder::default(), FrameTime::ZERO);
        root.paint(
            &mut Recorder::default(),
            FrameTime::from_nanos(1_000_000_000),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        let mut rec = Recorder::default();
        root.paint(&mut rec, FrameTime::from_nanos(2_000_000_000));
        // The opaque page paints its full-area black rect; the dialog paints its
        // scrim over it — so both a page fill and a scrim fill are present.
        assert!(
            rec.rects.iter().any(|(_, s, _)| *s == area),
            "the page below the transparent dialog is still painted"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, _, c)| *c == with_alpha(SCRIM, SCRIM_ALPHA)),
            "the dialog scrim is painted over the page"
        );
    }

    #[test]
    fn semantics_is_a_modal_dialog_labelled_by_title() {
        fn logic(_s: &mut ()) -> DialogView<()> {
            dialog().title("Confirm").body("Sure?")
        }
        let mut root: RenderRoot<(), DialogView<()>> = RenderRoot::new();
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
}
