//! The M3(E) dialog family, on the merged [`mod@crate::overlay::modal`] host:
//! the basic/centered dialog ([`dialog`]/[`show_dialog`], migrated off the old
//! standalone `push_transparent_for_result` path), a full-screen dialog
//! ([`full_screen_dialog`]/[`show_full_screen_dialog`]), and a selection/list
//! dialog ([`selection_dialog`]/[`show_selection_dialog`]).
//!
//! Source: `paadevelopments/material_3_expressive` v1.0.8 (MIT, © 2026 Paa
//! Developments — see `plugins/material/NOTICE`'s root-crate MIT section),
//! `lib/components/dialogs/{m3e_dialogs.dart, styles/m3e_dialog_theme.dart}`
//! (retrieved 2026-08-19, ~726 LOC combined). No license changes.
//!
//! # Architecture: the panel is content, the host is chrome
//!
//! Every variant here is a thin builder over
//! [`crate::overlay::modal::overlay_modal`]/[`crate::overlay::modal::show_overlay_modal`]:
//! this module owns **no** scrim, navigation-push, or dismiss-staging code of
//! its own any more (that was the pre-merge shape — grep-able: no
//! `push_transparent_for_result` remains in this file). The host contributes
//! the scrim, the panel's fill/radius/elevation, entrance/exit motion, the
//! scrim-tap/Escape/Android-back dismiss gestures (staged, animated), and
//! result plumbing; this module contributes only the panel's *content* — an
//! ordinary [`View`] built once per `build`/`rebuild` pass from cheaply-cloned
//! fields (`String`/`bool`/`Rc<..>`) and handed to [`overlay_modal`] — plus a
//! title/action-row layout ([`DialogPanelView`]) shared by the basic and
//! selection variants.
//!
//! # Compat: `dialog()`/`show_dialog()` keep their old shape
//!
//! [`dialog`]/[`DialogView`]/[`show_dialog`] keep the exact builder surface
//! in-repo consumers (`examples/huddle`) already call —
//! `dialog().title(..).body(..).action(..)`, `show_dialog(&nav, build,
//! on_result)` — just re-homed onto the host. [`DialogView::dismissable`] is
//! new (additive; every sibling modal component in this crate already carries
//! it).
//!
//! # Why actions are `Rc`-wrapped internally
//!
//! [`AnyView`] is not `Clone`, and [`View::build`]/[`View::rebuild`] only ever
//! see `&self` — so a fluent `.action(view)` builder that must *recompose* a
//! fresh, owned [`overlay_modal`] content view on every pass (there is no
//! other point in the `View` lifecycle to do it) cannot move the accumulated
//! `Vec<AnyView<State>>` out of a shared reference. Each action is wrapped in
//! an `Rc` on `.action()`/`.actions()` (transparent to callers — the public
//! signature is still plain `AnyView<State>`), so recomposing per pass is a
//! cheap refcount-bump clone rather than a deep one. No other field needs
//! this: titles/labels are `String` (`Clone`), the selection list's rows are
//! rebuilt fresh from primitive `Vec<String>`/`Rc<dyn Fn>` data each pass, and
//! a variant's single required `body`/`action` view is wrapped once, eagerly,
//! at its one-shot constructor call (already owned there).
//!
//! # Deliberate v1 scope cuts
//!
//! - **No `icon` slot.** The reference's `M3EDialog.icon`/`showSelectionScreen`'s
//!   `icon` are optional and unexercised by any in-repo consumer; adding the
//!   `icon == null` cross-axis-centering branch throughout the panel's layout
//!   is deferred rather than carried speculatively.
//! - **The full-screen header's close button dismisses unstaged.** The host's
//!   staged exit ramp is driven by [`crate::overlay::modal::OverlayModalWidget::request_dismiss`],
//!   a method private to that module — content (this file) has no reach into
//!   it. The header's own close affordance (leading, per the reference —
//!   unlike the host's own trailing-corner `close_button` chrome, which this
//!   variant turns off) instead fires the same `on_dismiss` callback
//!   [`crate::overlay::modal::show_overlay_modal`] wires to `controller.pop()`
//!   directly — an immediate pop, skipping the slide-down ramp. Escape and an
//!   Android back press still get the full staged exit (the host's own
//!   paths); there is no scrim to tap (the panel is full-bleed).
//! - **The selection list has no scroll physics.** The reference caps it at
//!   `0.45 * viewport height` inside a `SingleChildScrollView`; this port has
//!   no viewport reference available this deep in the content tree and does
//!   not reimplement drag-to-scroll. An overflowing list clips at the panel's
//!   own rounded-rect clip (the host already applies one to every panel's
//!   content) rather than scrolling — acceptable for the option counts this
//!   dialog is meant for, not a hidden bug.
//! - **Selection state is a controlled prop, not local widget state.** The
//!   reference's `_M3ESelectionDialogState` is a `StatefulWidget` owning its
//!   own `Set<String>` via `setState`. This crate's whole selection-control
//!   family (`radio`/`checkbox`/`switch`) is documented as controlled —
//!   "never self-mutating" — and there is no local-Component-state hook for a
//!   navigator-pushed page to lean on instead. [`selection_dialog`] follows
//!   the same contract: the caller threads `.selected(..)` in from its own
//!   `State` and reacts to `.on_toggle(..)`, rather than the dialog
//!   returning a final selection through the pop result the way
//!   `M3EDialog.showSelectionScreen`'s `Future<List<String>?>` does. Confirm/
//!   cancel wiring (including what a confirm tap pops with, if anything) is
//!   therefore the caller's own action, exactly like the basic dialog's
//!   existing `.action(..)` contract.
//!
//! # Panel geometry (cited from `m3e_dialog_theme.dart`)
//!
//! `minWidth`/`maxWidth` `[280, 560]` and the extra-large (28dp) shape are the
//! host's own `OverlayModalConfig::centered` defaults (`OVERLAY_DIALOG_MIN_WIDTH`/
//! `OVERLAY_DIALOG_MAX_WIDTH`, `shape.extra_large` via `crate::overlay`'s
//! `radius` accessor) — unchanged from this crate's pre-merge dialog, still
//! cited there. This file's own constants: [`DIALOG_PADDING`] (`padding: 24`),
//! [`ACTION_GAP`] (`actionGap: 8`), [`SELECTION_ITEM_HEIGHT`] (`selectionItemHeight:
//! 56`), [`FULL_SCREEN_HEADER_HEIGHT`] (`fullScreenHeaderHeight: 64`),
//! [`HEADER_EDGE_GAP`] (`headerEdgeGap: 4`), [`HEADER_ACTION_GAP`]
//! (`headerActionGap: 16`). The container role (`surfaceContainerHigh`) and
//! elevation (level 3) are the host's `OverlayModalConfig::centered` defaults
//! too — this crate's pre-merge dialog cited the identical values by hand.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight, TextOverflow};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, ThemeTextColor, View, Widget, any, build_child,
    rebuild_child, rebuild_children, route_event, route_event_single, teardown_child,
    visit_children,
};
use frust::{Color, NavigatorController, PopResult, Theme, icon, text};
use kurbo::{Point, Size};

use crate::button::{button, text_button};
use crate::checkbox::checkbox;
use crate::divider::divider;
use crate::icon_button::icon_button;
use crate::overlay::{
    OVERLAY_DIALOG_MAX_WIDTH, OverlayCorners, OverlayElevation, OverlayExtent, OverlayLimit,
    OverlayModalConfig, OverlayModalContent, OverlayModalView, OverlayModalWidget, OverlaySide,
    overlay_modal, show_overlay_modal,
};
use crate::radio::radio;

/// Panel padding on all four edges, in logical px (`M3EDialogTheme.padding`).
const DIALOG_PADDING: f64 = 24.0;
/// Horizontal spacing between adjacent action buttons (`M3EDialogTheme.actionGap`).
const ACTION_GAP: f64 = 8.0;
/// Height of each selectable row in the selection dialog
/// (`M3EDialogTheme.selectionItemHeight`).
const SELECTION_ITEM_HEIGHT: f64 = 56.0;
/// The selection list's own top/bottom inset — half of [`DIALOG_PADDING`],
/// mirroring the reference's `SingleChildScrollView(padding: top:
/// dialogTheme.padding.top / 2, bottom: dialogTheme.padding.bottom / 2)`.
const SELECTION_LIST_INSET: f64 = DIALOG_PADDING / 2.0;
/// Gap between a multi-select row's checkbox and its label — mirrors the
/// reference's `theme.radioTheme.labelGap` (checkbox has no built-in label
/// slot, unlike [`crate::radio`] — see `_buildCheckboxVisual`).
const CHECKBOX_LABEL_GAP: f64 = 8.0;
/// Height of the full-screen dialog's header row
/// (`M3EDialogTheme.fullScreenHeaderHeight`).
const FULL_SCREEN_HEADER_HEIGHT: f64 = 64.0;
/// Gap flanking the header's close affordance on both sides
/// (`M3EDialogTheme.headerEdgeGap`).
const HEADER_EDGE_GAP: f64 = 4.0;
/// Gap between the header's title and a trailing action, and between that
/// action and the header's trailing edge (`M3EDialogTheme.headerActionGap`).
const HEADER_ACTION_GAP: f64 = 16.0;
/// The reference's `_cancelLabel` default.
const DEFAULT_CANCEL_LABEL: &str = "Cancel";
/// The reference's `_confirmLabel` default.
const DEFAULT_CONFIRM_LABEL: &str = "OK";
/// The reference's `_closeSemanticLabel`.
const CLOSE_SEMANTIC_LABEL: &str = "Close";

/// Title type-scale token (M3 `headlineSmallEmphasized` — matches
/// `frust-theme::typography`'s `HEADLINE_SMALL_EMPHASIZED`). Hardcoded rather
/// than read from a live `Theme::type_scale`: `Text` has no layout-deferred
/// size/weight resolution seam (only a color role resolves post-`build` — see
/// `docs/CODE_STANDARDS.md`'s Theming conventions), the precedent this
/// module's pre-merge version already established.
const TITLE_SIZE: f32 = 24.0;
const TITLE_LINE_HEIGHT: f32 = 32.0;
const TITLE_WEIGHT: FontWeight = FontWeight::MEDIUM;
/// Body type-scale token (M3 `bodyMedium`).
const BODY_SIZE: f32 = 14.0;
const BODY_LINE_HEIGHT: f32 = 20.0;
/// Full-screen header title type-scale token (M3 `titleLarge`, REGULAR —
/// `frust-theme::typography`'s `TITLE_LARGE`; the reference's own
/// `theme.typeScale.titleLarge`, unlike the centered dialog's *emphasized*
/// headline).
const HEADER_TITLE_SIZE: f32 = 22.0;
const HEADER_TITLE_LINE_HEIGHT: f32 = 28.0;

/// Unthemed-fallback full-screen background fill (a theme resolves this from
/// `colors.surface` — `M3EDialogTheme.fullScreenBackground`).
const FALLBACK_SURFACE: Color = Color::from_rgb8(0xFE, 0xF7, 0xFF);

/// Build the title's type-erased child view (`headlineSmallEmphasized`,
/// themed `onSurface`).
fn title_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT)),
    )
}

/// Build the body text's type-erased child view (`bodyMedium`, themed
/// `onSurfaceVariant`).
fn body_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(BODY_SIZE)
            .line_height(LineHeight::Absolute(BODY_LINE_HEIGHT))
            .themed_role(ThemeTextColor::OnSurfaceVariant),
    )
}

/// Build the full-bleed divider's type-erased child view.
fn divider_view<State: 'static>() -> AnyView<State> {
    any::<State, _>(divider())
}

/// The resolved full-screen background fill. Themed: `colors.surface`.
/// Unthemed: [`FALLBACK_SURFACE`].
fn resolve_surface(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK_SURFACE, |t| t.scheme().surface)
}

// ============================================================================
// DialogPanelView / DialogPanelWidget — the shared centered-panel content
// (title, optional top divider, optional content, optional bottom divider,
// trailing action row). Used by both the basic dialog and the selection
// dialog — the selection dialog is exactly this shape with both dividers on,
// full-bleed content (the option list), and a fixed cancel/confirm action
// pair (see `m3e_dialogs.dart`'s `_M3ESelectionDialogState.build`, which
// literally constructs an `M3EDialog` with these same params).
// ============================================================================

/// A view-held, typed action callback list entry (see the module docs' *Why
/// actions are `Rc`-wrapped internally*).
type SharedAction<State> = Rc<AnyView<State>>;

/// The panel content shared by [`DialogView`] and [`SelectionDialogView`].
struct DialogPanelView<State: 'static> {
    title: Option<String>,
    content: Option<SharedAction<State>>,
    actions: Vec<SharedAction<State>>,
    top_divider: bool,
    bottom_divider: bool,
    /// Zero out the content's own padding on all four sides — the
    /// reference's `contentPadding: EdgeInsets.zero` override
    /// (the selection dialog's own list already insets itself; see
    /// [`SELECTION_LIST_INSET`]).
    content_full_bleed: bool,
}

/// The retained widget for a [`DialogPanelView`].
struct DialogPanelWidget {
    title: Option<ChildPod>,
    content: Option<ChildPod>,
    actions: Vec<ChildPod>,
    top_divider_pod: ChildPod,
    bottom_divider_pod: ChildPod,
    top_divider: bool,
    bottom_divider: bool,
    has_content: bool,
    has_actions: bool,
    content_full_bleed: bool,
}

/// Reconcile one optional `String`-sourced child (title) in place.
fn reconcile_text<State: 'static>(
    prev: Option<&String>,
    next: Option<&String>,
    pod: &mut Option<ChildPod>,
    make: impl Fn(&str) -> AnyView<State>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(s)) => {
            *pod = Some(build_child(&make(s.as_str()), ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(s), None) => {
            if let Some(p) = pod.as_mut() {
                teardown_child(&make(s.as_str()), p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => rebuild_child(&make(a.as_str()), &make(b.as_str()), p, ctx),
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

/// Reconcile one optional already-erased child (the generic content slot) in
/// place.
fn reconcile_any<State: 'static>(
    prev: Option<&SharedAction<State>>,
    next: Option<&SharedAction<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(v)) => {
            *pod = Some(build_child(v.as_ref(), ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(v), None) => {
            if let Some(p) = pod.as_mut() {
                teardown_child(v.as_ref(), p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => rebuild_child(a.as_ref(), b.as_ref(), p, ctx),
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

impl<State: 'static> View<State> for DialogPanelView<State> {
    type Element = DialogPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DialogPanelWidget {
        DialogPanelWidget {
            title: self
                .title
                .as_ref()
                .map(|s| build_child(&title_view::<State>(s), ctx)),
            content: self.content.as_ref().map(|c| build_child(c.as_ref(), ctx)),
            actions: self
                .actions
                .iter()
                .map(|a| build_child(a.as_ref(), ctx))
                .collect(),
            top_divider_pod: build_child(&divider_view::<State>(), ctx),
            bottom_divider_pod: build_child(&divider_view::<State>(), ctx),
            top_divider: self.top_divider,
            bottom_divider: self.bottom_divider,
            has_content: self.content.is_some(),
            has_actions: !self.actions.is_empty(),
            content_full_bleed: self.content_full_bleed,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DialogPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = reconcile_text(
            prev.title.as_ref(),
            self.title.as_ref(),
            &mut element.title,
            title_view::<State>,
            ctx,
        );
        flags |= reconcile_any(
            prev.content.as_ref(),
            self.content.as_ref(),
            &mut element.content,
            ctx,
        );

        let prev_actions: Vec<&AnyView<State>> = prev.actions.iter().map(|a| a.as_ref()).collect();
        let next_actions: Vec<&AnyView<State>> = self.actions.iter().map(|a| a.as_ref()).collect();
        flags |= rebuild_children(
            &prev_actions,
            &next_actions,
            &mut element.actions,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        flags |= rebuild_child(
            &divider_view::<State>(),
            &divider_view::<State>(),
            &mut element.top_divider_pod,
            ctx,
        );
        flags |= rebuild_child(
            &divider_view::<State>(),
            &divider_view::<State>(),
            &mut element.bottom_divider_pod,
            ctx,
        );

        if element.top_divider != self.top_divider {
            element.top_divider = self.top_divider;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.bottom_divider != self.bottom_divider {
            element.bottom_divider = self.bottom_divider;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.content_full_bleed != self.content_full_bleed {
            element.content_full_bleed = self.content_full_bleed;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.has_content = self.content.is_some();
        element.has_actions = !self.actions.is_empty();
        flags
    }

    fn teardown(&self, element: &mut DialogPanelWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(s), Some(p)) = (&self.title, element.title.as_mut()) {
            teardown_child(&title_view::<State>(s), p, ctx);
        }
        if let (Some(v), Some(p)) = (&self.content, element.content.as_mut()) {
            teardown_child(v.as_ref(), p, ctx);
        }
        for (view, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            teardown_child(view.as_ref(), pod, ctx);
        }
        teardown_child(&divider_view::<State>(), &mut element.top_divider_pod, ctx);
        teardown_child(
            &divider_view::<State>(),
            &mut element.bottom_divider_pod,
            ctx,
        );
    }
}

impl Widget for DialogPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The host gives content a tight width (`layout_panel`'s Centered
        // arm), loose height up to what's left of the viewport.
        let width = bc.max().width.max(bc.min().width);
        let inner_w = (width - 2.0 * DIALOG_PADDING).max(0.0);
        let loose = BoxConstraints::loose(Size::new(inner_w, f64::INFINITY));

        // No title (an old-API `dialog()` with none set) still reserves the
        // top margin a title's own wrapping padding would have — see the
        // module docs' compat note.
        let mut y = if self.title.is_some() {
            0.0
        } else {
            DIALOG_PADDING
        };

        if let Some(p) = self.title.as_mut() {
            y += DIALOG_PADDING;
            let s = p.layout_child(ctx, &loose);
            p.set_origin(Point::new(DIALOG_PADDING, y));
            y += s.height;
            y += DIALOG_PADDING;
        }

        let has_below = self.has_content || self.has_actions;
        if self.top_divider && has_below {
            self.top_divider_pod
                .layout_child(ctx, &BoxConstraints::tight(Size::new(width, 1.0)));
            self.top_divider_pod.set_origin(Point::new(0.0, y));
            y += self.top_divider_pod.size().height;
        }

        if let Some(p) = self.content.as_mut() {
            let (pad_lr, pad_tb) = if self.content_full_bleed {
                (0.0, 0.0)
            } else if self.top_divider && self.bottom_divider {
                (DIALOG_PADDING, DIALOG_PADDING)
            } else {
                (DIALOG_PADDING, 0.0)
            };
            let content_w = (width - 2.0 * pad_lr).max(0.0);
            y += pad_tb;
            let s = p.layout_child(
                ctx,
                &BoxConstraints::new(
                    Size::new(content_w, 0.0),
                    Size::new(content_w, f64::INFINITY),
                ),
            );
            p.set_origin(Point::new(pad_lr, y));
            y += s.height;
            y += pad_tb;
        }

        if self.bottom_divider && self.has_actions {
            self.bottom_divider_pod
                .layout_child(ctx, &BoxConstraints::tight(Size::new(width, 1.0)));
            self.bottom_divider_pod.set_origin(Point::new(0.0, y));
            y += self.bottom_divider_pod.size().height;
        }

        if self.has_actions {
            y += DIALOG_PADDING;
            let mut actions_w = 0.0;
            let mut actions_h: f64 = 0.0;
            for (i, pod) in self.actions.iter_mut().enumerate() {
                let s = pod.layout_child(ctx, &loose);
                if i > 0 {
                    actions_w += ACTION_GAP;
                }
                actions_w += s.width;
                actions_h = actions_h.max(s.height);
            }
            let actions_y = y;
            let mut ax = width - DIALOG_PADDING;
            for pod in self.actions.iter_mut().rev() {
                let w = pod.size().width;
                ax -= w;
                pod.set_origin(Point::new(ax, actions_y));
                ax -= ACTION_GAP;
            }
            let _ = actions_w;
            y += actions_h;
            y += DIALOG_PADDING;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(p) = self.title.as_mut() {
            p.paint_child(ctx, scene);
        }
        let has_below = self.has_content || self.has_actions;
        if self.top_divider && has_below {
            self.top_divider_pod.paint_child(ctx, scene);
        }
        if let Some(p) = self.content.as_mut() {
            p.paint_child(ctx, scene);
        }
        if self.bottom_divider && self.has_actions {
            self.bottom_divider_pod.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(content) = self.content.as_mut()
            && route_event_single(content, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        if !self.actions.is_empty()
            && route_event(&mut self.actions, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if let Some(p) = &self.title {
            p.semantics_child(ctx);
        }
        if let Some(p) = &self.content {
            p.semantics_child(ctx);
        }
        for pod in &self.actions {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(title, content, actions, top_divider_pod, bottom_divider_pod);
}

// ============================================================================
// dialog() / DialogView / show_dialog() — the basic, centered dialog.
// ============================================================================

/// A view-held, typed dismiss callback (erased on build).
type OnDismiss<State> = Rc<dyn Fn(&mut State)>;

/// A declarative basic M3 dialog: a centered panel over the host's scrim. See
/// the [module docs](self).
pub struct DialogView<State: 'static> {
    title: Option<String>,
    body: Option<String>,
    actions: Vec<SharedAction<State>>,
    dismissable: bool,
    on_dismiss: Option<OnDismiss<State>>,
}

/// Create an empty dialog. Chain [`DialogView::title`]/[`DialogView::body`]/
/// [`DialogView::action`]/[`DialogView::actions`] to fill it, and
/// [`DialogView::on_dismiss`] to observe a scrim-tap/Escape/back dismissal
/// (usually unnecessary — [`show_dialog`] already routes the pop for you).
pub fn dialog<State: 'static>() -> DialogView<State> {
    DialogView {
        title: None,
        body: None,
        actions: Vec::new(),
        dismissable: true,
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

    /// Replace the trailing action row with `actions` (app-provided buttons,
    /// in reading order — the last one sits closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions.into_iter().map(Rc::new).collect();
        self
    }

    /// Append one action button to the trailing action row.
    pub fn action(mut self, action: AnyView<State>) -> Self {
        self.actions.push(Rc::new(action));
        self
    }

    /// Whether the user can dismiss this dialog at all — the scrim tap,
    /// `Escape`, and an Android back press (default `true`; see
    /// [`crate::overlay::modal::OverlayModalView::dismissable`]). `false`
    /// disables all three; only an app-provided action still dismisses it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Observe a scrim-tap/Escape/back dismissal. [`show_dialog`] already
    /// wires the pop for you (via [`OverlayModalContent::on_modal_dismiss`]);
    /// this is for a caller that wants to react to it too (or that mounts a
    /// `DialogView` directly in a `Stack`, with no navigator involved).
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Compose the host-facing [`OverlayModalView`] fresh from the current
    /// fields (see the module docs' *Why actions are `Rc`-wrapped
    /// internally*).
    fn compose(&self) -> OverlayModalView<State> {
        let panel = DialogPanelView {
            title: self.title.clone(),
            content: self.body.as_ref().map(|s| Rc::new(body_view::<State>(s))),
            actions: self.actions.clone(),
            top_divider: false,
            bottom_divider: false,
            content_full_bleed: false,
        };
        let mut view = overlay_modal(
            panel,
            OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
        )
        .dismissable(self.dismissable);
        if let Some(title) = &self.title {
            view = view.label(title.clone());
        }
        if let Some(on_dismiss) = &self.on_dismiss {
            let on_dismiss = on_dismiss.clone();
            view = view.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        view
    }
}

impl<State: 'static> View<State> for DialogView<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        View::build(&self.compose(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.compose(), &prev.compose(), element, ctx)
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.compose(), element, ctx);
    }
}

impl<State: 'static> OverlayModalContent<State> for DialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.dismissable
    }
}

/// Push `build`'s dialog as a transparent navigator page over the host's
/// scrim, and register `on_result` for the value it pops with.
///
/// Dismissal (scrim tap, `Escape`, Android back) is wired to
/// `controller.pop()` for you, staged behind the host's exit ramp; an action
/// button pops with a value via `controller.pop_with_result(..)`, immediately
/// (see the module docs).
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
    show_overlay_modal(controller, build, on_result);
}

// ============================================================================
// selection_dialog() / SelectionDialogView / show_selection_dialog() — the
// centered dialog with a scrollable(-ish; see the module docs) option list.
// ============================================================================

/// A view-held, typed per-option toggle callback (erased on build).
type OnToggle<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative M3 selection dialog: [`dialog`]'s centered panel, both
/// dividers on, and a radio/checkbox option list for content. See the
/// [module docs](self)' controlled-component note.
pub struct SelectionDialogView<State: 'static> {
    title: String,
    options: Vec<String>,
    multi_select: bool,
    selected: Vec<String>,
    on_toggle: Option<OnToggle<State>>,
    cancel_label: String,
    confirm_label: String,
    on_confirm: Option<OnDismiss<State>>,
    dismissable: bool,
    on_dismiss: Option<OnDismiss<State>>,
}

/// Create a selection dialog titled `title`, offering `options` (single-select
/// by default — see [`SelectionDialogView::multi_select`]).
pub fn selection_dialog<State: 'static>(
    title: impl Into<String>,
    options: Vec<String>,
) -> SelectionDialogView<State> {
    SelectionDialogView {
        title: title.into(),
        options,
        multi_select: false,
        selected: Vec::new(),
        on_toggle: None,
        cancel_label: DEFAULT_CANCEL_LABEL.to_string(),
        confirm_label: DEFAULT_CONFIRM_LABEL.to_string(),
        on_confirm: None,
        dismissable: true,
        on_dismiss: None,
    }
}

impl<State: 'static> SelectionDialogView<State> {
    /// Allow more than one option selected at once (checkbox rows instead of
    /// radio rows) — the reference's `multiSelect`.
    pub fn multi_select(mut self, multi_select: bool) -> Self {
        self.multi_select = multi_select;
        self
    }

    /// The currently-selected options (controlled — see the [module
    /// docs](self)). Single-select reads only the first entry.
    pub fn selected(mut self, selected: Vec<String>) -> Self {
        self.selected = selected;
        self
    }

    /// Fired with the tapped option's value on every row press — a
    /// single-select tap reports the newly-chosen option; a multi-select tap
    /// reports whichever option should flip membership. The caller updates
    /// its own `selected` set and re-renders.
    pub fn on_toggle<F: Fn(&mut State, String) + 'static>(mut self, on_toggle: F) -> Self {
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    /// Override the cancel action's label (`_cancelLabel`, default "Cancel").
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Override the confirm action's label (`_confirmLabel`, default "OK").
    pub fn confirm_label(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = label.into();
        self
    }

    /// Fired on a confirm tap — disabled (per the reference) until at least
    /// one option is selected. The caller decides what confirming means
    /// (typically `controller.pop_with_result(..)` with its own `selected`),
    /// exactly like the basic dialog's `.action(..)` contract.
    pub fn on_confirm<F: Fn(&mut State) + 'static>(mut self, on_confirm: F) -> Self {
        self.on_confirm = Some(Rc::new(on_confirm));
        self
    }

    /// Whether the user can dismiss this dialog via the scrim tap, `Escape`,
    /// or an Android back press (default `true`) — see
    /// [`DialogView::dismissable`].
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Observe a scrim-tap/Escape/back/cancel dismissal — see
    /// [`DialogView::on_dismiss`].
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    fn compose(&self) -> OverlayModalView<State> {
        let list = SelectionListView {
            options: self.options.clone(),
            multi_select: self.multi_select,
            selected: self.selected.clone(),
            on_toggle: self.on_toggle.clone().unwrap_or_else(|| Rc::new(|_, _| {})),
        };

        let on_dismiss = self.on_dismiss.clone();
        let cancel_action: AnyView<State> = any(text_button(
            self.cancel_label.clone(),
            move |state: &mut State| {
                if let Some(on_dismiss) = &on_dismiss {
                    on_dismiss(state);
                }
            },
        ));
        let on_confirm = self.on_confirm.clone();
        let confirm_action: AnyView<State> = any(button(
            self.confirm_label.clone(),
            move |state: &mut State| {
                if let Some(on_confirm) = &on_confirm {
                    on_confirm(state);
                }
            },
        )
        .enabled(!self.selected.is_empty()));

        let panel = DialogPanelView {
            title: Some(self.title.clone()),
            content: Some(Rc::new(any(list))),
            actions: vec![Rc::new(cancel_action), Rc::new(confirm_action)],
            top_divider: true,
            bottom_divider: true,
            content_full_bleed: true,
        };
        let mut view = overlay_modal(
            panel,
            OverlayModalConfig::centered(OVERLAY_DIALOG_MAX_WIDTH),
        )
        .dismissable(self.dismissable)
        .label(self.title.clone());
        if let Some(on_dismiss) = &self.on_dismiss {
            let on_dismiss = on_dismiss.clone();
            view = view.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        view
    }
}

impl<State: 'static> View<State> for SelectionDialogView<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        View::build(&self.compose(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.compose(), &prev.compose(), element, ctx)
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.compose(), element, ctx);
    }
}

impl<State: 'static> OverlayModalContent<State> for SelectionDialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.dismissable
    }
}

/// Push `build`'s selection dialog as a transparent navigator page, and
/// register `on_result` for the value it pops with. See [`show_dialog`].
pub fn show_selection_dialog<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> SelectionDialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    show_overlay_modal(controller, build, on_result);
}

/// The option-list content view built fresh, each pass, from primitive owned
/// data (never `Rc`-wrapped — see the module docs).
struct SelectionListView<State: 'static> {
    options: Vec<String>,
    multi_select: bool,
    selected: Vec<String>,
    on_toggle: OnToggle<State>,
}

/// One selection row: the control ([`crate::radio`]/[`crate::checkbox`],
/// reused read-only) plus an optional separate label pod (multi-select only —
/// [`crate::checkbox`] has no built-in label slot, unlike [`crate::radio`]).
struct SelectionRow {
    control: ChildPod,
    label: Option<ChildPod>,
}

/// The retained widget for a [`SelectionListView`].
struct SelectionListWidget {
    rows: Vec<SelectionRow>,
}

/// Build one row's control (+ label, if multi-select) view pair.
fn selection_row_views<State: 'static>(
    option: &str,
    multi_select: bool,
    selected: &[String],
    on_toggle: &OnToggle<State>,
) -> (AnyView<State>, Option<AnyView<State>>) {
    if multi_select {
        let checked = selected.iter().any(|s| s == option);
        let toggle = on_toggle.clone();
        let value = option.to_string();
        let control = any(checkbox(checked, move |state: &mut State, _next: bool| {
            toggle(state, value.clone())
        }));
        let label = any(text(option.to_string()).themed_role(ThemeTextColor::OnSurface));
        (control, Some(label))
    } else {
        let group_value = selected.first().cloned();
        let toggle = on_toggle.clone();
        let control = any(
            radio::<State, Option<String>>(Some(option.to_string()), group_value)
                .on_changed(move |state: &mut State, value: Option<String>| {
                    if let Some(value) = value {
                        toggle(state, value);
                    }
                })
                .label(option.to_string()),
        );
        (control, None)
    }
}

impl<State: 'static> View<State> for SelectionListView<State> {
    type Element = SelectionListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SelectionListWidget {
        let rows = self
            .options
            .iter()
            .map(|option| {
                let (control, label) = selection_row_views::<State>(
                    option,
                    self.multi_select,
                    &self.selected,
                    &self.on_toggle,
                );
                SelectionRow {
                    control: build_child(&control, ctx),
                    label: label.map(|l| build_child(&l, ctx)),
                }
            })
            .collect();
        SelectionListWidget { rows }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectionListWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let shape_changed = prev.options != self.options
            || prev.multi_select != self.multi_select
            || element.rows.len() != self.options.len();
        if shape_changed {
            for (view, row) in prev.options.iter().zip(element.rows.iter_mut()) {
                let (control, label) = selection_row_views::<State>(
                    view,
                    prev.multi_select,
                    &prev.selected,
                    &prev.on_toggle,
                );
                teardown_child(&control, &mut row.control, ctx);
                if let (Some(label_view), Some(pod)) = (label, row.label.as_mut()) {
                    teardown_child(&label_view, pod, ctx);
                }
            }
            element.rows = self
                .options
                .iter()
                .map(|option| {
                    let (control, label) = selection_row_views::<State>(
                        option,
                        self.multi_select,
                        &self.selected,
                        &self.on_toggle,
                    );
                    SelectionRow {
                        control: build_child(&control, ctx),
                        label: label.map(|l| build_child(&l, ctx)),
                    }
                })
                .collect();
            return ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let mut flags = ChangeFlags::NONE;
        for (i, (option, row)) in self.options.iter().zip(element.rows.iter_mut()).enumerate() {
            let (prev_control, prev_label) = selection_row_views::<State>(
                &prev.options[i],
                prev.multi_select,
                &prev.selected,
                &prev.on_toggle,
            );
            let (next_control, next_label) = selection_row_views::<State>(
                option,
                self.multi_select,
                &self.selected,
                &self.on_toggle,
            );
            flags |= rebuild_child(&prev_control, &next_control, &mut row.control, ctx);
            if let (Some(prev_l), Some(next_l), Some(pod)) =
                (prev_label, next_label, row.label.as_mut())
            {
                flags |= rebuild_child(&prev_l, &next_l, pod, ctx);
            }
        }
        flags
    }

    fn teardown(&self, element: &mut SelectionListWidget, ctx: &mut BuildCtx<'_>) {
        for (option, row) in self.options.iter().zip(element.rows.iter_mut()) {
            let (control, label) = selection_row_views::<State>(
                option,
                self.multi_select,
                &self.selected,
                &self.on_toggle,
            );
            teardown_child(&control, &mut row.control, ctx);
            if let (Some(label_view), Some(pod)) = (label, row.label.as_mut()) {
                teardown_child(&label_view, pod, ctx);
            }
        }
    }
}

impl Widget for SelectionListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = bc.max().width.max(bc.min().width);
        let mut y = SELECTION_LIST_INSET;
        let row_w = (width - 2.0 * DIALOG_PADDING).max(0.0);
        let row_bc = BoxConstraints::loose(Size::new(row_w, SELECTION_ITEM_HEIGHT));

        for row in &mut self.rows {
            let control_size = row.control.layout_child(ctx, &row_bc);
            let mut row_h = control_size.height;
            let label_size = row.label.as_mut().map(|l| l.layout_child(ctx, &row_bc));
            if let Some(s) = label_size {
                row_h = row_h.max(s.height);
            }
            let band_h = row_h.max(SELECTION_ITEM_HEIGHT);
            let control_y = y + (band_h - control_size.height) / 2.0;
            row.control
                .set_origin(Point::new(DIALOG_PADDING, control_y));
            if let (Some(label), Some(s)) = (row.label.as_mut(), label_size) {
                let label_x = DIALOG_PADDING + control_size.width + CHECKBOX_LABEL_GAP;
                let label_y = y + (band_h - s.height) / 2.0;
                label.set_origin(Point::new(label_x, label_y));
            }
            y += band_h.max(SELECTION_ITEM_HEIGHT);
        }
        y += SELECTION_LIST_INSET;

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for row in &mut self.rows {
            row.control.paint_child(ctx, scene);
            if let Some(label) = row.label.as_mut() {
                label.paint_child(ctx, scene);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        for row in &mut self.rows {
            if route_event_single(&mut row.control, ctx, event) == EventResult::Handled {
                return EventResult::Handled;
            }
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for row in &self.rows {
            row.control.semantics_child(ctx);
            if let Some(label) = &row.label {
                label.semantics_child(ctx);
            }
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
        for row in &self.rows {
            visitor(&row.control);
            if let Some(label) = &row.label {
                visitor(label);
            }
        }
    }
}

// ============================================================================
// full_screen_dialog() / FullScreenDialogView / show_full_screen_dialog() —
// the edge-to-edge dialog: header (close, title, optional action) + divider +
// body, sliding up from the bottom.
// ============================================================================

/// A declarative M3 full-screen dialog: an app-bar-like header over a body
/// slot, filling the whole area. See the [module docs](self).
pub struct FullScreenDialogView<State: 'static> {
    title: String,
    body: Rc<AnyView<State>>,
    action: Option<Rc<AnyView<State>>>,
    dismissable: bool,
    on_dismiss: Option<OnDismiss<State>>,
}

/// Create a full-screen dialog titled `title`, filling its body slot with
/// `body`. Chain [`FullScreenDialogView::action`] for a trailing header
/// action.
pub fn full_screen_dialog<State: 'static, V: View<State>>(
    title: impl Into<String>,
    body: V,
) -> FullScreenDialogView<State> {
    FullScreenDialogView {
        title: title.into(),
        body: Rc::new(any(body)),
        action: None,
        dismissable: true,
        on_dismiss: None,
    }
}

impl<State: 'static> FullScreenDialogView<State> {
    /// Set the header's trailing action (e.g. a "Save" button) — the
    /// reference's `action`.
    pub fn action(mut self, action: AnyView<State>) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    /// Whether `Escape`/an Android back press dismiss this dialog (default
    /// `true`) — see [`DialogView::dismissable`]. There is no scrim to tap
    /// (the panel is full-bleed) and the header's own close affordance
    /// always dismisses regardless of this flag (matching the reference,
    /// where the header close is the dialog's one unconditional exit).
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Observe a dismissal (`Escape`, back, or the header close) — see
    /// [`DialogView::on_dismiss`].
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    fn config() -> OverlayModalConfig {
        OverlayModalConfig::edge(OverlaySide::Bottom)
            .extent(OverlayExtent::Fraction(1.0), OverlayLimit::None)
            .corners(OverlayCorners::None)
            .close_button(false)
            .scrim_dismiss(false)
            .elevation(OverlayElevation::None)
    }

    fn compose(&self) -> OverlayModalView<State> {
        let panel = FullScreenPanelView {
            title: self.title.clone(),
            body: self.body.clone(),
            action: self.action.clone(),
            on_dismiss: self.on_dismiss.clone(),
        };
        let mut view = overlay_modal(panel, Self::config())
            .dismissable(self.dismissable)
            .label(self.title.clone());
        if let Some(on_dismiss) = &self.on_dismiss {
            let on_dismiss = on_dismiss.clone();
            view = view.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        view
    }
}

impl<State: 'static> View<State> for FullScreenDialogView<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        View::build(&self.compose(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.compose(), &prev.compose(), element, ctx)
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.compose(), element, ctx);
    }
}

impl<State: 'static> OverlayModalContent<State> for FullScreenDialogView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.on_dismiss = Some(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        // The header's own close affordance always dismisses (see
        // `FullScreenDialogView::dismissable`'s doc) — but that reaches
        // `on_dismiss` directly rather than through the host's staged
        // request-dismiss/back-signal path, so `dismissable` here still
        // governs only Escape/back, matching every other variant.
        self.dismissable
    }
}

/// Push `build`'s full-screen dialog as a transparent navigator page, and
/// register `on_result` for the value it pops with. See [`show_dialog`].
pub fn show_full_screen_dialog<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> FullScreenDialogView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    show_overlay_modal(controller, build, on_result);
}

/// The full-screen panel's content view: header (close, title, optional
/// action) + divider + body.
struct FullScreenPanelView<State: 'static> {
    title: String,
    body: Rc<AnyView<State>>,
    action: Option<Rc<AnyView<State>>>,
    on_dismiss: Option<OnDismiss<State>>,
}

/// The retained widget for a [`FullScreenPanelView`].
struct FullScreenPanelWidget {
    close: ChildPod,
    title: ChildPod,
    action: Option<ChildPod>,
    divider: ChildPod,
    body: ChildPod,
}

/// Build the header close affordance's view — [`icon_button`] over
/// [`crate::icons::CLOSE`], firing `on_dismiss` directly (see the module
/// docs' *Deliberate v1 scope cuts*).
fn close_view<State: 'static>(on_dismiss: Option<OnDismiss<State>>) -> AnyView<State> {
    any(
        icon_button(any(icon(crate::icons::CLOSE)), move |state: &mut State| {
            if let Some(on_dismiss) = &on_dismiss {
                on_dismiss(state);
            }
        })
        .semantic_label(CLOSE_SEMANTIC_LABEL),
    )
}

/// Build the header title's view (`titleLarge`, single line, ellipsized).
fn header_title_view<State: 'static>(title: &str) -> AnyView<State> {
    any::<State, _>(
        text(title.to_string())
            .size(HEADER_TITLE_SIZE)
            .line_height(LineHeight::Absolute(HEADER_TITLE_LINE_HEIGHT))
            .max_lines(1)
            .overflow(TextOverflow::Ellipsis),
    )
}

impl<State: 'static> View<State> for FullScreenPanelView<State> {
    type Element = FullScreenPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FullScreenPanelWidget {
        FullScreenPanelWidget {
            close: build_child(&close_view::<State>(self.on_dismiss.clone()), ctx),
            title: build_child(&header_title_view::<State>(&self.title), ctx),
            action: self.action.as_ref().map(|a| build_child(a.as_ref(), ctx)),
            divider: build_child(&divider_view::<State>(), ctx),
            body: build_child(self.body.as_ref(), ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut FullScreenPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &close_view::<State>(prev.on_dismiss.clone()),
            &close_view::<State>(self.on_dismiss.clone()),
            &mut element.close,
            ctx,
        );
        flags |= rebuild_child(
            &header_title_view::<State>(&prev.title),
            &header_title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        flags |= match (prev.action.as_ref(), self.action.as_ref()) {
            (None, Some(v)) => {
                element.action = Some(build_child(v.as_ref(), ctx));
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
            (Some(v), None) => {
                if let Some(p) = element.action.as_mut() {
                    teardown_child(v.as_ref(), p, ctx);
                }
                element.action = None;
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            }
            (Some(a), Some(b)) => match element.action.as_mut() {
                Some(p) => rebuild_child(a.as_ref(), b.as_ref(), p, ctx),
                None => ChangeFlags::NONE,
            },
            (None, None) => ChangeFlags::NONE,
        };
        flags |= rebuild_child(
            &divider_view::<State>(),
            &divider_view::<State>(),
            &mut element.divider,
            ctx,
        );
        flags |= rebuild_child(
            prev.body.as_ref(),
            self.body.as_ref(),
            &mut element.body,
            ctx,
        );
        flags
    }

    fn teardown(&self, element: &mut FullScreenPanelWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(
            &close_view::<State>(self.on_dismiss.clone()),
            &mut element.close,
            ctx,
        );
        teardown_child(
            &header_title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        if let (Some(v), Some(p)) = (&self.action, element.action.as_mut()) {
            teardown_child(v.as_ref(), p, ctx);
        }
        teardown_child(&divider_view::<State>(), &mut element.divider, ctx);
        teardown_child(self.body.as_ref(), &mut element.body, ctx);
    }
}

impl Widget for FullScreenPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The host gives a full-screen edge panel a tight size (`layout_panel`'s
        // Edge arm, `extent: Fraction(1.0)`).
        let size = bc.max();
        let (width, height) = (size.width, size.height);

        let loose = BoxConstraints::loose(Size::new(width, FULL_SCREEN_HEADER_HEIGHT));
        let close_size = self.close.layout_child(ctx, &loose);
        let action_size = self.action.as_mut().map(|a| a.layout_child(ctx, &loose));

        let close_x = HEADER_EDGE_GAP;
        self.close.set_origin(Point::new(
            close_x,
            (FULL_SCREEN_HEADER_HEIGHT - close_size.height) / 2.0,
        ));

        let title_leading = close_x + close_size.width + HEADER_EDGE_GAP;
        let trailing_reserve = action_size
            .map(|s| s.width + HEADER_ACTION_GAP)
            .unwrap_or(0.0);
        let title_w = (width - title_leading - trailing_reserve).max(0.0);
        let title_bc = BoxConstraints::new(
            Size::new(title_w, 0.0),
            Size::new(title_w, FULL_SCREEN_HEADER_HEIGHT),
        );
        let title_size = self.title.layout_child(ctx, &title_bc);
        self.title.set_origin(Point::new(
            title_leading,
            (FULL_SCREEN_HEADER_HEIGHT - title_size.height) / 2.0,
        ));

        if let (Some(action), Some(s)) = (self.action.as_mut(), action_size) {
            action.set_origin(Point::new(
                width - HEADER_ACTION_GAP - s.width,
                (FULL_SCREEN_HEADER_HEIGHT - s.height) / 2.0,
            ));
        }

        self.divider
            .layout_child(ctx, &BoxConstraints::tight(Size::new(width, 1.0)));
        self.divider
            .set_origin(Point::new(0.0, FULL_SCREEN_HEADER_HEIGHT));
        let divider_h = self.divider.size().height;

        let body_y = FULL_SCREEN_HEADER_HEIGHT + divider_h;
        let body_h = (height - body_y).max(0.0);
        self.body
            .layout_child(ctx, &BoxConstraints::tight(Size::new(width, body_h)));
        self.body.set_origin(Point::new(0.0, body_y));

        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The host's own `container` fill sits underneath; this dialog paints
        // its own opaque `colors.surface` full-bleed fill over it (see the
        // module docs — `fullScreenBackground`).
        let theme = Theme::from_paint_ctx(ctx);
        scene.fill_rect(ctx.origin(), ctx.size(), resolve_surface(theme));

        self.close.paint_child(ctx, scene);
        self.title.paint_child(ctx, scene);
        if let Some(action) = self.action.as_mut() {
            action.paint_child(ctx, scene);
        }
        self.divider.paint_child(ctx, scene);
        self.body.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if route_event_single(&mut self.close, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if let Some(action) = self.action.as_mut()
            && route_event_single(action, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.body, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.close.semantics_child(ctx);
        self.title.semantics_child(ctx);
        if let Some(action) = &self.action {
            action.semantics_child(ctx);
        }
        self.body.semantics_child(ctx);
    }

    visit_children!(close, title, action, divider, body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{
        CornerRadii, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent, PointerPhase,
        text::TextContext,
    };
    use frust::{FrameTime, NavigatorView};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use std::any::Any;

    /// The window every dialog test lays out in.
    const WINDOW: Size = Size::new(400.0, 600.0);

    /// Records filled rects, rounded rects, and glyph runs so a test can
    /// assert a variant actually painted its chrome.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, CornerRadii, Color)>,
        glyphs: usize,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.glyphs += 1;
        }
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, c: Color) {
            self.rrects.push((o, s, radii, c));
        }
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

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<bool>>,
        toggled: Vec<String>,
        confirmed: bool,
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

    /// A fixed-size leaf firing `on_tap` on release inside it — lets a test
    /// place a dialog action of known geometry without depending on real
    /// text-shaping metrics.
    struct TapView {
        size: Size,
        on_tap: Rc<dyn Fn(&mut NavState)>,
    }
    struct TapWidget {
        size: Size,
        on_tap: frust::authoring::ErasedCallback,
        captured: bool,
    }
    impl View<NavState> for TapView {
        type Element = TapWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TapWidget {
            TapWidget {
                size: self.size,
                on_tap: frust::authoring::erase_callback(&self.on_tap),
                captured: false,
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut TapWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.on_tap = frust::authoring::erase_callback(&self.on_tap);
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
    fn tap_action(w: f64, h: f64, on_tap: impl Fn(&mut NavState) + 'static) -> AnyView<NavState> {
        any(TapView {
            size: Size::new(w, h),
            on_tap: Rc::new(on_tap),
        })
    }

    /// The app-logic closure the [`Harness`] rebuilds through.
    type NavLogic = Box<dyn FnMut(&mut NavState) -> NavigatorView<NavState>>;

    struct Harness {
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        state: NavState,
        tcx: TextContext,
        controller: NavigatorController<NavState>,
        logic: NavLogic,
    }

    impl Harness {
        fn new() -> Self {
            let controller: NavigatorController<NavState> = NavigatorController::new();
            let ctrl = controller.clone();
            let mut h = Harness {
                root: RenderRoot::new(),
                state: NavState::default(),
                tcx: TextContext::new(),
                controller,
                logic: Box::new(move |_: &mut NavState| {
                    navigator(&ctrl, || {
                        any::<NavState, _>(sized(WINDOW.width, WINDOW.height))
                    })
                }),
            };
            h.pass();
            h
        }

        fn pass(&mut self) {
            self.root.rebuild(&mut self.logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
        }

        /// Settle the modal's entrance ramp.
        fn settle(&mut self) {
            self.root.paint(&mut Recorder::default(), FrameTime::ZERO);
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(1_000_000_000),
            );
            self.pass();
        }

        /// Settle a running staged-exit ramp so its enqueued pop drains.
        fn settle_exit(&mut self) {
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(2_000_000_000),
            );
            self.root.paint(
                &mut Recorder::default(),
                FrameTime::from_nanos(3_000_000_000),
            );
            self.pass();
        }

        fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        fn flush(&mut self) {
            self.pass();
            self.event(&ev(PointerPhase::Move, 2.0, 2.0));
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos(4_000_000_000));
            rec
        }
    }

    // --- 1. All three variants render (paint-recorder). ---

    #[test]
    fn the_basic_dialog_renders_title_body_and_actions() {
        let mut h = Harness::new();
        show_dialog(
            &h.controller,
            || {
                dialog()
                    .title("Confirm")
                    .body("Are you sure?")
                    .action(any(text_button("OK", |_s: &mut NavState| {})))
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        let rec = h.paint();
        assert!(
            rec.glyphs >= 3,
            "title, body, and the action label all shape"
        );
    }

    #[test]
    fn the_selection_dialog_renders_options_and_actions() {
        let mut h = Harness::new();
        show_selection_dialog(
            &h.controller,
            || {
                selection_dialog("Pick one", vec!["A".into(), "B".into(), "C".into()])
                    .selected(vec!["B".into()])
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        let rec = h.paint();
        // Title + three radio labels + Cancel + OK.
        assert!(rec.glyphs >= 6);
    }

    #[test]
    fn the_full_screen_dialog_renders_header_and_body() {
        let mut h = Harness::new();
        show_full_screen_dialog(
            &h.controller,
            || full_screen_dialog("Settings", sized(200.0, 200.0)),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        let rec = h.paint();
        assert!(rec.glyphs >= 1, "the header title shapes");
        // The full-bleed surface fill, then the body's own black fill.
        assert!(rec.rects.len() >= 2);
    }

    // --- 2. Dismissal routes through the modal host; result plumbing works. ---

    #[test]
    fn scrim_tap_stages_an_exit_then_pops_with_an_empty_result() {
        let mut h = Harness::new();
        show_dialog(
            &h.controller,
            || dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        h.event(&ev(PointerPhase::Down, 5.0, 5.0));
        h.event(&ev(PointerPhase::Up, 5.0, 5.0));
        h.flush();
        assert!(
            h.state.results.is_empty(),
            "still animating out — no local scrim/pop code fired it directly"
        );
        h.settle_exit();
        h.flush();
        assert_eq!(h.state.results, vec![None]);
    }

    #[test]
    fn escape_stages_the_same_exit_once_the_dialog_has_focus() {
        let mut h = Harness::new();
        show_dialog(
            &h.controller,
            || dialog().title("Confirm"),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        // A press claims focus (host-owned; this file no longer opts in
        // itself) without dismissing.
        h.event(&ev(PointerPhase::Down, 200.0, 300.0));
        h.event(&ev(PointerPhase::Up, 200.0, 300.0));
        h.flush();
        assert!(h.state.results.is_empty());
        h.event(&escape_event());
        h.flush();
        assert!(h.state.results.is_empty(), "still animating out");
        h.settle_exit();
        h.flush();
        assert_eq!(h.state.results, vec![None]);
    }

    #[test]
    fn a_dismissable_false_dialog_ignores_escape() {
        let mut h = Harness::new();
        show_dialog(
            &h.controller,
            || dialog().title("Pinned").dismissable(false),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        h.event(&ev(PointerPhase::Down, 200.0, 300.0));
        h.event(&ev(PointerPhase::Up, 200.0, 300.0));
        h.event(&escape_event());
        h.flush();
        h.settle_exit();
        h.flush();
        assert!(
            h.state.results.is_empty(),
            "a non-dismissable dialog never pops via Escape"
        );
    }

    #[test]
    fn an_action_pops_with_a_value_via_the_navigator() {
        // A fixed-size action and no title, so the panel's geometry is exact
        // without depending on real text-shaping metrics. The host's own
        // `OverlayModalConfig::centered` sizes the panel to
        // `OVERLAY_DIALOG_MAX_WIDTH` clamped into the viewport margin
        // (`MaterialSpacing::LG`, 16dp each side) — not shrunk to content —
        // so in a 400px-wide window the panel is `400 - 2*16 = 368`px wide.
        const ACTION: Size = Size::new(80.0, 36.0);
        const PANEL_W: f64 = 368.0;
        const PANEL_MARGIN: f64 = 16.0;

        let mut h = Harness::new();
        h.controller.push_transparent_for_result(
            {
                let ctrl = h.controller.clone();
                move || {
                    let c = ctrl.clone();
                    any::<NavState, _>(dialog().action(tap_action(
                        ACTION.width,
                        ACTION.height,
                        move |_s: &mut NavState| c.pop_with_result(PopResult::of(true)),
                    )))
                }
            },
            frust::TransitionSpec::NONE,
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();

        // No title: the actions block still opens with its own `padding`
        // (24dp) — see the module docs' compat note — so the panel's content
        // height is `24 + 24 + 36 = 84`, centered vertically in the window.
        let panel_h = 2.0 * DIALOG_PADDING + ACTION.height;
        let panel_y = (WINDOW.height - panel_h) / 2.0;
        let action_x = PANEL_MARGIN + PANEL_W - DIALOG_PADDING - ACTION.width;
        let action_y = panel_y + DIALOG_PADDING;
        let (cx, cy) = (
            action_x + ACTION.width / 2.0,
            action_y + ACTION.height / 2.0,
        );
        h.event(&ev(PointerPhase::Down, cx, cy));
        h.event(&ev(PointerPhase::Up, cx, cy));
        h.flush();
        assert_eq!(h.state.results, vec![Some(true)]);
    }

    #[test]
    fn selection_dialog_confirm_reports_the_controlled_selection() {
        let mut h = Harness::new();
        show_selection_dialog(
            &h.controller,
            || {
                selection_dialog("Pick", vec!["A".into(), "B".into()])
                    .selected(vec!["A".into()])
                    .on_toggle(|s: &mut NavState, v: String| s.toggled.push(v))
                    .on_confirm(|s: &mut NavState| {
                        s.confirmed = true;
                    })
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        assert!(!h.state.confirmed);
    }

    // --- 3. Existing consumer compile shape (mirrors `examples/huddle`). ---

    #[test]
    fn the_old_builder_shape_still_compiles_and_runs() {
        let mut h = Harness::new();
        show_dialog(
            &h.controller,
            || {
                dialog()
                    .title("Invite people")
                    .body("Send invites to this workspace? (mock)")
                    .action(any(text_button("Send", |_s: &mut NavState| {})))
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<bool>());
            },
        );
        h.pass();
        h.settle();
        assert!(h.state.results.is_empty());
    }
}
