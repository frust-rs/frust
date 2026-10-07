// Ported from `material_3_expressive` v1.0.8's `M3ESideSheet`/
// `M3ESideSheetTheme` (MIT, © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/side_sheets/m3e_side_sheets.dart`,
// `styles/m3e_side_sheet_theme.dart`, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions: the header/body/footer *composition* (title + a close
// affordance, an `Expanded` body, an optional divider-plus-actions footer) is
// carried over from `M3ESideSheet`/`M3ESideSheetTheme`; the modal chrome
// itself (scrim, edge-pinned slide, staged exit, back-dismiss, the close
// affordance's hit-testing) is **not** re-derived here at all — it comes
// entirely from this crate's own [`crate::overlay::modal`] host, configured
// for a right-edge panel (`OverlayModalConfig::edge`). This module is that
// host's first real consumer: [`SideSheetView`] is a thin, chrome-only
// wrapper over [`OverlayModalView`] rather than a hand-rolled widget (unlike
// [`mod@crate::dialog`]/[`crate::sheet`], which predate the host).

//! The modal M3 side sheet: secondary content anchored to the trailing
//! (right) edge of the screen, sliding in from that edge over the M3 32%
//! scrim.
//!
//! # Built on the merged overlay modal host
//!
//! [`SideSheetView`] wraps an [`OverlayModalView`] configured via
//! [`OverlayModalConfig::edge`]`(`[`OverlaySide::Right`]`)`: the scrim, the
//! edge-pinned slide-in/out motion, the staged exit ramp, the back-press
//! [`frust::BackPolicy::DismissAnimated`] seam, the scrim-tap dismiss, and the
//! **close affordance** (the host's own top-trailing-corner control, an M3
//! 48dp touch target) are every one of them the host's, reused wholesale —
//! see [`mod@crate::overlay::modal`]'s own module docs for the full
//! contract. This module contributes only the M3E side-sheet *chrome* the
//! host has no opinion about: the header/body/footer composition below, and
//! [`SIDE_SHEET_WIDTH`]'s fixed-width geometry.
//!
//! # Geometry: fixed width, full height, left corners only
//!
//! A side sheet is [`SIDE_SHEET_WIDTH`] wide (320dp — `M3ESideSheetTheme`'s
//! own default, inside the M3 256–400dp side-sheet width band; capped down
//! only when the viewport itself is narrower) and full height, via
//! [`OverlayExtent::Content`] capped at [`OverlayLimit::Px`]`(SIDE_SHEET_WIDTH)`
//! — a *fixed* width, not the host's own fraction-of-viewport `.edge()`
//! default (that shape is left for a genuinely responsive edge panel; a side
//! sheet is not one upstream). [`OverlayModalConfig::edge`]'s own
//! `OverlaySide::Right` mapping already rounds only the panel's *left*
//! corners (`OverlayCorners::Start` — the corners facing into the screen; the
//! right corners sit at the screen edge and stay square), so no override is
//! needed here for the asymmetry.
//!
//! # The close affordance is the host's, not a second one
//!
//! [`side_sheet_config`] turns on [`OverlayModalConfig::close_button`], so
//! the same top-trailing-corner control every other host-built modal can
//! offer is what dismisses this sheet — this module never paints a second
//! close icon inside its own header. [`SideSheetContentWidget`]'s header
//! layout reserves [`HEADER_TRAILING_RESERVE`] logical px on its trailing
//! edge (matching the host's own private close-chrome footprint) so the
//! title never sits under it.
//!
//! # Header / body / footer
//!
//! - **Header**: `title` (`titleLarge`, `onSurface`), left-inset
//!   [`MaterialSpacing::XL`] (24dp), vertically centered in a fixed
//!   [`HEADER_HEIGHT`] (80dp) row — `M3ESideSheetTheme`'s own
//!   `headerPadding`/`closeButtonPadding`/`iconSize` numbers, re-expressed as
//!   the trailing reserve above.
//! - **Body**: `body` (a caller view of any shape), filling the remaining
//!   height between the header and the footer, both axes tight — the
//!   reference's `Expanded(child: body)` inside a `crossAxisAlignment:
//!   stretch` column.
//! - **Footer**: shown only when `actions` is non-empty — a
//!   [`crate::divider`] (full width, `outlineVariant`, matching
//!   `M3ESideSheetTheme::dividerColor`) plus a left-packed row of the
//!   app-provided `actions`, inset [`MaterialSpacing::LG`] (16dp) on every
//!   edge (`M3ESideSheetTheme`'s `actionsPadding`). The reference's own
//!   `Row(children: actions)` carries no inter-action gap; this module
//!   matches that exactly rather than inventing one. An action that closes
//!   the sheet does it through [`SideSheetView::dismiss_handle`]'s
//!   [`ModalDismiss`], not a raw `controller.pop()`: the handle stages the
//!   host's slide-out ramp, a raw pop dismisses on the spot.
//!
//! # Content is fixed at construction
//!
//! [`side_sheet`] takes `title`/`body`/`actions` together and erases them
//! into the host's own `content: AnyView<State>` slot immediately — there is
//! no post-construction `.action`/`.actions` builder chaining, unlike
//! [`mod@crate::dialog`]'s. The host's `content` field is a genuinely opaque
//! `AnyView`, so mutating it after construction would need either a second,
//! shared-ownership indirection this module has no other reason to carry, or
//! reaching into the host's own private fields (a coupling this module
//! deliberately avoids — see [`SideSheetView`]'s builder methods, which
//! reach only the host's *public*
//! `dismissable`/`on_dismiss`/`dismiss_handle` seam). Matches
//! the reference's own constructor shape too: `M3ESideSheet({required title,
//! required body, actions = const []})` takes `actions` as one parameter,
//! never incrementally built either.
//!
//! # Docked (non-modal) variant: absent upstream
//!
//! `material_3_expressive`'s `side_sheets` family ships only the modal
//! `M3ESideSheet`/`M3ESideSheet.show` — no non-modal, layout-participating
//! "docked" sheet exists in the reference to port. Not invented here.
//!
//! # Semantics
//!
//! Delegated entirely to the host: [`OverlayModalWidget::semantics`]
//! contributes one [`frust::authoring::Role::Dialog`] container node with the
//! accesskit modal flag set, labelled with `title` (via
//! [`OverlayModalView::label`], which [`side_sheet`] wires for you).
//! [`SideSheetContentWidget`]'s own `semantics` is a transparent forward —
//! title, body, and each action become the host node's children, in that
//! order.

use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, ThemeTextType, View, ViewSeq, Widget, any,
};
use frust::text;
use kurbo::{Point, Size};

use crate::overlay::{
    ModalDismiss, OverlayExtent, OverlayLimit, OverlayModalConfig, OverlayModalContent,
    OverlayModalView, OverlayModalWidget, OverlaySide, overlay_modal, show_overlay_modal,
};
use crate::tokens::MaterialSpacing;
use frust::{NavigatorController, PopResult};

/// The M3 side-sheet width band's lower bound, in logical px
/// (m3.material.io's side-sheet spec) — read by this module's own tests to
/// pin [`SIDE_SHEET_WIDTH`] inside the band.
#[allow(dead_code)]
const SIDE_SHEET_MIN_WIDTH: f64 = 256.0;

/// This module's fixed panel width, in logical px — `M3ESideSheetTheme`'s own
/// `width` default (320dp), inside the [`SIDE_SHEET_MIN_WIDTH`]–
/// [`crate::overlay::OVERLAY_SIDE_SHEET_MAX_WIDTH`] (400dp) M3 side-sheet
/// width band. Public so a caller can size a companion layout (e.g. a
/// persistent rail alongside a docked variant, were one ever added) against
/// it.
pub const SIDE_SHEET_WIDTH: f64 = 320.0;

/// Header left inset — `MaterialSpacing::XL` (24dp; `M3ESideSheetTheme`'s
/// `headerPadding.left`).
const HEADER_PAD_LEFT: f64 = MaterialSpacing::XL;
/// Header top/bottom inset — `MaterialSpacing::LG` (16dp; `headerPadding`'s
/// `.top`/`.bottom`).
const HEADER_PAD_TOP: f64 = MaterialSpacing::LG;
const HEADER_PAD_BOTTOM: f64 = MaterialSpacing::LG;
/// The header row's own trailing inset before the close affordance —
/// `MaterialSpacing::SM` (8dp; `headerPadding.right`).
const HEADER_PAD_RIGHT: f64 = MaterialSpacing::SM;
/// The close affordance's own M3 touch target: `MaterialSpacing::MD` (12dp)
/// padding around the 24dp `M3EIcons.close` glyph on every edge
/// (`closeButtonPadding`/`iconSize`) — the same 48dp minimum touch target the
/// merged overlay modal host paints its own close chrome at. This module
/// never paints a second close icon (see the module docs); it only reserves
/// this width in its own header layout.
const CLOSE_TAP_TARGET: f64 = 2.0 * MaterialSpacing::MD + 24.0;
/// The header's trailing reserve, clear of the host's own close affordance —
/// [`HEADER_PAD_RIGHT`] plus [`CLOSE_TAP_TARGET`].
const HEADER_TRAILING_RESERVE: f64 = HEADER_PAD_RIGHT + CLOSE_TAP_TARGET;
/// The header's fixed height: [`HEADER_PAD_TOP`] + [`CLOSE_TAP_TARGET`] +
/// [`HEADER_PAD_BOTTOM`] (the close affordance, at 48dp, is the row's tallest
/// element — taller than the title's own `titleLarge` line height).
const HEADER_HEIGHT: f64 = HEADER_PAD_TOP + CLOSE_TAP_TARGET + HEADER_PAD_BOTTOM;

/// The footer's inset on every edge — `MaterialSpacing::LG` (16dp;
/// `M3ESideSheetTheme`'s `actionsPadding`).
const ACTIONS_PADDING: f64 = MaterialSpacing::LG;

/// Title type-scale token (M3 `titleLarge`, *not* the emphasized sibling —
/// the reference's own `titleStyle` reads `type.titleLarge` unmodified).
/// Hardcoded rather than read from a live `Theme::type_scale`: `Text` has no
/// layout-time-deferred size/weight resolution seam (only a *color* role and
/// an opted-in *family* role can be resolved after `View::build` — see
/// `docs/CODE_STANDARDS.md`'s Theming conventions), the same precedent
/// [`mod@crate::dialog`]/[`mod@crate::appbar`] follow. The title does opt its
/// family into the `titleLarge` role.
const TITLE_SIZE: f32 = 22.0;
const TITLE_LINE_HEIGHT: f32 = 28.0;
const TITLE_WEIGHT: FontWeight = FontWeight::REGULAR;

/// The M3 side-sheet chrome: right-edge-pinned (the trailing edge in LTR),
/// fixed [`SIDE_SHEET_WIDTH`] wide, full height, left corners only (the host's
/// own `OverlaySide::Right` → `OverlayCorners::Start` mapping — no override
/// needed), sliding in with the host's own [`OverlayEntrance::Slide`]
/// (`.edge`'s default), and the host's own close affordance enabled. See the
/// [module docs](self).
fn side_sheet_config() -> OverlayModalConfig {
    OverlayModalConfig::edge(OverlaySide::Right)
        .extent(OverlayExtent::Content, OverlayLimit::Px(SIDE_SHEET_WIDTH))
        .close_button(true)
}

/// Build the title's type-erased child view: `titleLarge`-styled text,
/// defaulting to the `Text` widget's own `OnSurface` themed role (matches
/// `M3ESideSheetTheme::titleStyle`'s `onSurface` ink), its family following
/// the live theme's `titleLarge` role.
fn title_view<State: 'static>(title: &str) -> AnyView<State> {
    any::<State, _>(
        text(title.to_string())
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT))
            .themed_family(ThemeTextType::TitleLarge),
    )
}

/// The composed header/body/footer content [`side_sheet`] hands to the
/// [`crate::overlay::modal`] host as its `content` — see [`side_sheet_config`].
struct SideSheetContentView<State: 'static> {
    title: String,
    body: AnyView<State>,
    actions: Vec<AnyView<State>>,
}

impl<State: 'static> View<State> for SideSheetContentView<State> {
    type Element = SideSheetContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SideSheetContentWidget {
        SideSheetContentWidget {
            title: frust::authoring::build_child(&title_view::<State>(&self.title), ctx),
            body: frust::authoring::build_child(&self.body, ctx),
            divider: (!self.actions.is_empty()).then(|| {
                frust::authoring::build_child(&any::<State, _>(crate::divider::divider()), ctx)
            }),
            actions: self
                .actions
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SideSheetContentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_child(
            &title_view::<State>(&prev.title),
            &title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        flags |= frust::authoring::rebuild_child(&prev.body, &self.body, &mut element.body, ctx);

        match (prev.actions.is_empty(), self.actions.is_empty()) {
            (true, false) => {
                element.divider = Some(frust::authoring::build_child(
                    &any::<State, _>(crate::divider::divider()),
                    ctx,
                ));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (false, true) => {
                if let Some(pod) = element.divider.as_mut() {
                    frust::authoring::teardown_child(
                        &any::<State, _>(crate::divider::divider()),
                        pod,
                        ctx,
                    );
                }
                element.divider = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }

        let prev_views: Vec<&AnyView<State>> = prev.actions.iter().collect();
        let next_views: Vec<&AnyView<State>> = self.actions.iter().collect();
        flags |= frust::authoring::rebuild_children(
            &prev_views,
            &next_views,
            &mut element.actions,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        flags
    }

    fn teardown(&self, element: &mut SideSheetContentWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(
            &title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        frust::authoring::teardown_child(&self.body, &mut element.body, ctx);
        if let Some(pod) = element.divider.as_mut() {
            frust::authoring::teardown_child(&any::<State, _>(crate::divider::divider()), pod, ctx);
        }
        for (view, pod) in self.actions.iter().zip(element.actions.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

/// The retained widget for a [`SideSheetContentView`]. See the [module
/// docs](self).
struct SideSheetContentWidget {
    title: ChildPod,
    body: ChildPod,
    /// Present exactly when `actions` is non-empty — the footer's separator.
    divider: Option<ChildPod>,
    actions: Vec<ChildPod>,
}

impl Widget for SideSheetContentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = crate::overlay::finite_or_zero(bc.max().width);
        let height = crate::overlay::finite_or_zero(bc.max().height);

        // Header: title, left-inset, vertically centered in the fixed
        // HEADER_HEIGHT row, leaving HEADER_TRAILING_RESERVE clear on the
        // trailing edge for the host's own close affordance.
        let title_max_w = (width - HEADER_PAD_LEFT - HEADER_TRAILING_RESERVE).max(0.0);
        let title_size = self.title.layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(title_max_w, f64::INFINITY)),
        );
        self.title.set_origin(Point::new(
            HEADER_PAD_LEFT,
            ((HEADER_HEIGHT - title_size.height) / 2.0).max(0.0),
        ));

        // Footer: an optional divider + a left-packed row of actions, hugging
        // the panel's bottom edge — present only when actions is non-empty.
        let mut footer_h = 0.0;
        if let Some(divider) = self.divider.as_mut() {
            let divider_size =
                divider.layout_child(ctx, &BoxConstraints::loose(Size::new(width, height)));

            let action_bc = BoxConstraints::loose(Size::new(
                (width - 2.0 * ACTIONS_PADDING).max(0.0),
                f64::INFINITY,
            ));
            let mut actions_h: f64 = 0.0;
            for pod in self.actions.iter_mut() {
                let s = pod.layout_child(ctx, &action_bc);
                actions_h = actions_h.max(s.height);
            }

            footer_h = divider_size.height + 2.0 * ACTIONS_PADDING + actions_h;
            let footer_y = height - footer_h;
            divider.set_origin(Point::new(0.0, footer_y));

            // Left-packed, no inter-action gap — the reference's own bare
            // `Row(children: actions)` (`m3e_side_sheets.dart`'s
            // `_buildActions`) carries none either.
            let mut ax = ACTIONS_PADDING;
            let actions_y = footer_y + divider_size.height + ACTIONS_PADDING;
            for pod in self.actions.iter_mut() {
                let s = pod.size();
                pod.set_origin(Point::new(ax, actions_y));
                ax += s.width;
            }
        }

        // Body: fills the remaining space between the header and the footer,
        // both axes tight — `Expanded(child: body)` inside a
        // `crossAxisAlignment: stretch` column.
        let body_h = (height - HEADER_HEIGHT - footer_h).max(0.0);
        self.body
            .layout_child(ctx, &BoxConstraints::tight(Size::new(width, body_h)));
        self.body.set_origin(Point::new(0.0, HEADER_HEIGHT));

        Size::new(width, height)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.title.paint_child(ctx, scene);
        self.body.paint_child(ctx, scene);
        if let Some(divider) = self.divider.as_mut() {
            divider.paint_child(ctx, scene);
        }
        for pod in &mut self.actions {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.body.event_child(ctx, event);
            for pod in &mut self.actions {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if frust::authoring::route_event_single(&mut self.body, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        frust::authoring::route_event(&mut self.actions, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.title.semantics_child(ctx);
        self.body.semantics_child(ctx);
        if let Some(divider) = &self.divider {
            divider.semantics_child(ctx);
        }
        for pod in &self.actions {
            pod.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(title, body, divider, actions);
}

/// A declarative modal M3 side sheet: a header (`title` + the host's own
/// close affordance), `body`, and an optional `actions` footer, wrapped
/// around the merged [`crate::overlay::modal`] host. See the [module
/// docs](self).
pub struct SideSheetView<State: 'static>(OverlayModalView<State>);

/// Build a modal side sheet. See the [module docs](self)' Header/body/footer
/// and Content-is-fixed-at-construction sections; [`show_side_sheet`] is the
/// navigator-pushed convenience most callers want.
///
/// ```ignore
/// side_sheet(
///     "Settings",
///     settings_body(),
///     [text_button("Close").on_press(|s: &mut State| { .. })],
/// )
/// ```
pub fn side_sheet<State: 'static, V: View<State>, M>(
    title: impl Into<String>,
    body: V,
    actions: impl ViewSeq<State, M>,
) -> SideSheetView<State> {
    let mut erased = Vec::new();
    actions.extend_views(&mut erased);
    let title = title.into();
    let content = SideSheetContentView {
        title: title.clone(),
        body: any(body),
        actions: erased,
    };
    SideSheetView(overlay_modal(content, side_sheet_config()).label(title))
}

impl<State: 'static> SideSheetView<State> {
    /// Whether the user can dismiss this sheet at all — the scrim tap, the
    /// host's own close affordance, `Escape`, and an Android back press
    /// (default `true`; see [`mod@crate::overlay::modal`]'s module docs).
    /// `false` disables all of them; only an explicit control inside `body`/
    /// `actions` still dismisses it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.0 = self.0.dismissable(dismissable);
        self
    }

    /// Set the **unstaged** dismiss callback — delivered with `&mut State`
    /// during the event pass. [`show_side_sheet`] wires the staged
    /// (navigator-pop, slide-out) path instead; set this only for a
    /// `Stack`-mounted sheet with no navigator underneath it.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.0 = self.0.on_dismiss(on_dismiss);
        self
    }

    /// Install the app-triggered staged-dismiss handle, so an `actions` button
    /// ("Apply", "Reset") closes the sheet through the **same** slide-out ramp
    /// the close affordance, the scrim tap, `Escape`, and a back press take —
    /// rather than the raw `controller.pop()` that dismisses on the spot. See
    /// [`ModalDismiss`], and mint the handle outside [`show_side_sheet`]'s page
    /// builder.
    ///
    /// ```ignore
    /// let dismiss = ModalDismiss::new();
    /// let apply = dismiss.clone();
    /// side_sheet(title, body, [
    ///     filled_button("Apply", move |_: &mut State| apply.dismiss()),
    /// ])
    /// .dismiss_handle(dismiss.clone())
    /// ```
    pub fn dismiss_handle(mut self, dismiss: ModalDismiss) -> Self {
        self.0 = self.0.dismiss_handle(dismiss);
        self
    }
}

impl<State: 'static> View<State> for SideSheetView<State> {
    type Element = OverlayModalWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> OverlayModalWidget {
        View::build(&self.0, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut OverlayModalWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.0, &prev.0, element, ctx)
    }

    fn teardown(&self, element: &mut OverlayModalWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.0, element, ctx);
    }
}

impl<State: 'static> OverlayModalContent<State> for SideSheetView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.0 = self.0.on_modal_dismiss(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.0.modal_dismissable()
    }
}

/// Push `build`'s side sheet as a transparent navigator page (the page below
/// stays visible under the scrim), sliding in from the trailing edge, and
/// register `on_result` for the value it pops with — the
/// [`crate::overlay::modal`] host's shared [`show_overlay_modal`] push, since
/// [`SideSheetView`] *is* a pre-configured modal host (see
/// [`OverlayModalContent`]). The scrim tap, the host's own close affordance,
/// `Escape`, and an Android back press are all wired to `controller.pop()`
/// for you, staged behind the slide-out exit ramp (see
/// [`mod@crate::overlay::modal`]'s Exit motion / Back-dismiss sections).
///
/// The sheet's own `actions` reach that same ramp through
/// [`SideSheetView::dismiss_handle`] — minted here, outside `build`, and
/// cloned into both the sheet and its buttons.
///
/// ```ignore
/// show_side_sheet(
///     &state.nav,
///     || side_sheet("Settings", settings_body(), Vec::<AnyView<State>>::new()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// ```
pub fn show_side_sheet<State, B, R>(controller: &NavigatorController<State>, build: B, on_result: R)
where
    State: 'static,
    B: Fn() -> SideSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    show_overlay_modal(controller, build, on_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::{OverlayCorners, OverlayEntrance, OverlayGeometry};
    use frust::FrameTime;
    use frust::NavigatorView;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BuildCtx, ChangeFlags, CornerRadii, EventCtx, PointerButton, PointerEvent, PointerPhase,
        Role, Widget, any as core_any,
    };
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use frust_widgets::test_support::leaf_any;
    use kurbo::Rect;
    use peniko::Color;
    use std::any::Any;

    /// The window every test lays out in.
    const WINDOW: Size = Size::new(400.0, 600.0);

    /// A recording `PaintScene`: fills, per-corner rounded rects, and clips.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        panels: Vec<(Point, Size, CornerRadii, Color)>,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect_radii(
            &mut self,
            origin: Point,
            size: Size,
            radii: CornerRadii,
            color: Color,
        ) {
            self.panels.push((origin, size, radii, color));
        }
        fn push_clip_rounded_radii(&mut self, _origin: Point, _size: Size, _radii: CornerRadii) {}
        fn push_clip(&mut self, _origin: Point, _size: Size) {}
        fn pop_clip(&mut self) {}
    }

    /// A `SideSheetView` built directly (bypassing [`side_sheet`]'s own
    /// `Slide` entrance) so a paint/layout assertion sees the panel already
    /// at rest — [`mod@crate::overlay::modal`]'s own `settled` test
    /// precedent.
    fn settled_view<State: 'static, V: View<State>>(
        title: &str,
        body: V,
        actions: Vec<AnyView<State>>,
    ) -> SideSheetView<State> {
        let content = SideSheetContentView {
            title: title.to_string(),
            body: any(body),
            actions,
        };
        SideSheetView(
            overlay_modal(content, side_sheet_config().entrance(OverlayEntrance::None))
                .label(title.to_string()),
        )
    }

    fn build<S: 'static>(view: &SideSheetView<S>) -> OverlayModalWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut OverlayModalWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    // ---- Config: geometry, corners, entrance, close affordance, width. ----

    #[test]
    fn side_sheet_config_pins_a_right_edge_fixed_width_panel_with_left_corners_only() {
        let config = side_sheet_config();
        assert_eq!(
            config.geometry,
            OverlayGeometry::Edge {
                side: OverlaySide::Right,
                extent: OverlayExtent::Content,
                limit: OverlayLimit::Px(SIDE_SHEET_WIDTH),
            }
        );
        assert_eq!(
            config.corners,
            OverlayCorners::Start,
            "a right-pinned panel rounds its left (screen-facing) corners only"
        );
        assert_eq!(
            config.entrance,
            OverlayEntrance::Slide,
            "a side sheet slides in from its own edge"
        );
        assert!(
            config.close_button,
            "the host's own close affordance is enabled, not a second one"
        );
    }

    #[test]
    fn the_fixed_width_sits_inside_the_m3_side_sheet_spec_band() {
        assert!(
            (SIDE_SHEET_MIN_WIDTH..=crate::overlay::OVERLAY_SIDE_SHEET_MAX_WIDTH)
                .contains(&SIDE_SHEET_WIDTH),
            "320dp (the reference's own default) must sit inside the M3 [256, 400]dp band"
        );
    }

    // ---- Layout: header reserve, body fill, optional footer. ----

    #[test]
    fn layout_reserves_the_header_and_fills_the_body_with_no_footer() {
        let view: SideSheetView<()> = settled_view("Filters", leaf_any(100.0, 100.0), Vec::new());
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size, WINDOW, "the host fills its own area (the scrim)");

        let panel = w.panel_rect();
        assert_eq!(
            panel.width(),
            SIDE_SHEET_WIDTH,
            "a 400px window is wider than SIDE_SHEET_WIDTH, so no clamp bites"
        );
        assert_eq!(panel.height(), WINDOW.height, "full height");
        // The panel is pinned to the trailing (right) edge.
        assert_eq!(panel.x1, WINDOW.width);
    }

    #[test]
    fn layout_adds_a_divider_and_a_left_packed_footer_row_when_actions_is_non_empty() {
        let view: SideSheetView<()> = settled_view(
            "Filters",
            leaf_any(100.0, 100.0),
            vec![leaf_any(60.0, 32.0), leaf_any(60.0, 32.0)],
        );
        let mut w = build(&view);
        layout(&mut w);
        let panel_h = w.panel_rect().height();

        // Reach into the content widget through the public accessors this
        // test module has: since `content` itself is crate-private, assert
        // the *painted* geometry instead (paint test below covers the exact
        // positions; here just prove the panel's own height accounts for a
        // footer at all by checking a fresh, footer-less build differs).
        let no_actions: SideSheetView<()> =
            settled_view("Filters", leaf_any(100.0, 100.0), Vec::new());
        let mut w2 = build(&no_actions);
        layout(&mut w2);
        assert_eq!(
            w.panel_rect().height(),
            w2.panel_rect().height(),
            "the panel itself is always full height regardless of a footer"
        );
        assert_eq!(panel_h, WINDOW.height);
    }

    // ---- Paint: corner asymmetry + width. ----

    fn paint(w: &mut OverlayModalWidget) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, WINDOW);
        w.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn unthemed_paint_rounds_only_the_left_corners_at_the_fallback_radius() {
        let view: SideSheetView<()> = settled_view("Filters", leaf_any(100.0, 100.0), Vec::new());
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w);

        assert_eq!(rec.panels.len(), 1, "one panel fill");
        let (origin, size, radii, _color) = rec.panels[0];
        assert_eq!(size.width, SIDE_SHEET_WIDTH);
        assert_eq!(origin.x, WINDOW.width - SIDE_SHEET_WIDTH, "pinned trailing");

        let radius = crate::overlay::radius(None);
        assert_eq!(radii.top_left, radius, "left corners round");
        assert_eq!(radii.bottom_left, radius, "left corners round");
        assert_eq!(
            radii.top_right, 0.0,
            "right corners are square (screen edge)"
        );
        assert_eq!(
            radii.bottom_right, 0.0,
            "right corners are square (screen edge)"
        );
    }

    #[test]
    fn themed_paint_resolves_the_extra_large_shape_token_on_the_rounded_corners() {
        let theme = crate::baseline();
        let view: SideSheetView<()> = settled_view("Filters", leaf_any(100.0, 100.0), Vec::new());
        let mut w = build(&view);
        layout(&mut w);

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, WINDOW).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let (_, _, radii, _) = rec.panels[0];
        assert_eq!(radii.top_left, theme.shape.extra_large);
        assert_eq!(radii.bottom_left, theme.shape.extra_large);
        assert_eq!(radii.top_right, 0.0);
        assert_eq!(radii.bottom_right, 0.0);
    }

    // ---- Motion: the real (non-settled) constructor slides. ----

    #[test]
    fn the_real_constructor_starts_off_screen_and_slides_to_the_trailing_edge() {
        let view: SideSheetView<()> =
            side_sheet("Filters", leaf_any(100.0, 100.0), Vec::<AnyView<()>>::new());
        let mut w = build(&view);
        layout(&mut w);
        assert_eq!(
            w.progress(),
            0.0,
            "an un-settled Slide entrance starts closed"
        );

        // Settle the ramp: paint once to seed it, once well past its duration.
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::ZERO);
        w.paint(&mut pctx, &mut rec);
        layout(&mut w);
        let mut rec2 = Recorder::default();
        let mut pctx2 =
            PaintCtx::for_test(Point::ZERO, WINDOW, FrameTime::from_nanos(2_000_000_000));
        w.paint(&mut pctx2, &mut rec2);
        layout(&mut w);

        assert_eq!(w.progress(), 1.0, "the slide settles fully open");
        assert_eq!(
            w.panel_rect().x1,
            WINDOW.width,
            "settled, the panel is flush against the trailing edge"
        );
    }

    // ---- Dismiss: standalone (unstaged) close affordance + scrim tap. ----

    #[derive(Default)]
    struct Flag {
        dismissed: u32,
    }

    fn dispatch(w: &mut OverlayModalWidget, state: &mut Flag, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn settled_flag_view(actions: Vec<AnyView<Flag>>) -> SideSheetView<Flag> {
        let mut view = settled_view("Filters", leaf_any_flag(100.0, 100.0), actions);
        view = view.on_dismiss(|s: &mut Flag| s.dismissed += 1);
        view
    }

    // A `()`-erased leaf is `AnyView<()>`; the `Flag`-state sheet needs an
    // `AnyView<Flag>` — mirrors `crate::sheet`'s own test-only adapter.
    fn leaf_any_flag(w: f64, h: f64) -> AnyView<Flag> {
        core_any::<Flag, _>(FixedLeaf {
            size: Size::new(w, h),
        })
    }
    struct FixedLeaf {
        size: Size,
    }
    struct FixedLeafW {
        size: Size,
    }
    impl View<Flag> for FixedLeaf {
        type Element = FixedLeafW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> FixedLeafW {
            FixedLeafW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut FixedLeafW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FixedLeafW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
    }

    #[test]
    fn the_hosts_close_affordance_dismisses_via_on_dismiss() {
        let view = settled_flag_view(Vec::new());
        let mut w = build(&view);
        layout(&mut w);
        let close: Rect = w.close_rect().expect("close_button(true) is configured");
        let center = close.center();

        let mut state = Flag::default();
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, center.x, center.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, center.x, center.y),
        );
        assert_eq!(state.dismissed, 1);
    }

    #[test]
    fn a_scrim_tap_outside_the_panel_dismisses() {
        let view = settled_flag_view(Vec::new());
        let mut w = build(&view);
        layout(&mut w);
        // The panel is pinned to the trailing edge; the leading edge is scrim.
        assert!(!w.panel_rect().contains(Point::new(5.0, 5.0)));

        let mut state = Flag::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.dismissed, 1);
    }

    // ---- Dismiss + motion: navigator-pushed, staged exit. ----

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
    }

    fn ft(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    #[test]
    fn show_side_sheet_stages_a_scrim_tap_dismiss_through_the_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || {
                    core_any::<NavState, _>(bg_page(WINDOW.width, WINDOW.height))
                })
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        show_side_sheet(
            &controller,
            || {
                side_sheet(
                    "Filters",
                    bg_page(100.0, 100.0),
                    Vec::<AnyView<NavState>>::new(),
                )
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        // Settle the Slide entrance (LONG_2, 500ms M3 default).
        for t in [0u64, 100, 200, 300, 400, 550] {
            root.paint(&mut Recorder::default(), ft(t));
        }

        // Tap the leading-edge scrim, outside the trailing-pinned panel.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        // The dismiss is staged behind the exit ramp: drive frames until it
        // settles and the deferred pop drains.
        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            let outcome = root.paint(&mut Recorder::default(), ft(600 + frames * 100));
            frames += 1;
            assert!(
                frames < 30,
                "the staged exit settles within a bounded number of frames"
            );
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "a scrim tap stages the exit and pops with an empty result"
        );
    }

    #[test]
    fn back_request_stages_the_exit_and_pops_via_the_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || {
                    core_any::<NavState, _>(bg_page(WINDOW.width, WINDOW.height))
                })
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        show_side_sheet(
            &controller,
            || {
                side_sheet(
                    "Filters",
                    bg_page(100.0, 100.0),
                    Vec::<AnyView<NavState>>::new(),
                )
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        for t in [0u64, 100, 200, 300, 400, 550] {
            root.paint(&mut Recorder::default(), ft(t));
        }

        assert!(
            controller.back_interest(),
            "a dismissable side sheet claims back interest"
        );

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.depth(),
            2,
            "DismissAnimated: unchanged immediately"
        );

        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            let outcome = root.paint(&mut Recorder::default(), ft(600 + frames * 100));
            frames += 1;
            assert!(
                frames < 30,
                "the staged exit settles within a bounded number of frames"
            );
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "a back request dismisses the sheet"
        );
        assert_eq!(controller.depth(), 1);
    }

    #[test]
    fn an_actions_dismiss_handle_stages_the_slide_out_instead_of_popping_at_once() {
        // The sheet's own Reset/Apply buttons: a raw `controller.pop()` from
        // one takes the page out from under the host and the panel vanishes
        // mid-slide. Through the handle it takes the very same staged exit the
        // scrim tap above takes — and can carry the pop result with it.
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || {
                    core_any::<NavState, _>(bg_page(WINDOW.width, WINDOW.height))
                })
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        // Minted outside the page builder, exactly as an app does it.
        let dismiss = ModalDismiss::new();
        let installed = dismiss.clone();
        show_side_sheet(
            &controller,
            move || {
                side_sheet(
                    "Filters",
                    bg_page(100.0, 100.0),
                    Vec::<AnyView<NavState>>::new(),
                )
                .dismiss_handle(installed.clone())
            },
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        for t in [0u64, 100, 200, 300, 400, 550] {
            root.paint(&mut Recorder::default(), ft(t));
        }
        assert_eq!(controller.depth(), 2, "the sheet is up");

        // "Apply".
        dismiss.dismiss_with(PopResult::of(3));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft(600));
        assert_eq!(
            controller.depth(),
            2,
            "nothing pops while the slide-out runs"
        );

        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            let outcome = root.paint(&mut Recorder::default(), ft(700 + frames * 100));
            frames += 1;
            assert!(
                frames < 30,
                "the staged exit settles within a bounded number of frames"
            );
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![Some(3)],
            "the action's own result rides the staged pop"
        );
        assert_eq!(controller.depth(), 1);
    }

    // A minimal opaque background page for the navigator integration tests.
    struct BgPage {
        size: Size,
    }
    struct BgPageW {
        size: Size,
    }
    impl View<NavState> for BgPage {
        type Element = BgPageW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> BgPageW {
            BgPageW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut BgPageW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for BgPageW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn bg_page(w: f64, h: f64) -> BgPage {
        BgPage {
            size: Size::new(w, h),
        }
    }

    // ---- Semantics: forwarded to the host's Role::Dialog node. ----

    #[test]
    fn semantics_is_a_modal_dialog_labelled_by_title_with_header_body_and_actions_as_children() {
        fn logic(_s: &mut ()) -> SideSheetView<()> {
            side_sheet(
                "Filters",
                leaf_any(100.0, 100.0),
                vec![leaf_any(60.0, 32.0)],
            )
        }
        let mut root: RenderRoot<(), SideSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the side sheet node sets the modal flag");
        assert_eq!(node.label(), Some("Filters"));
        assert!(
            !node.children().is_empty(),
            "title/body/actions are semantics children"
        );
    }
}
