// Ported from `material_3_expressive` v1.0.8's search view
// (`lib/components/search/m3e_search_anchor.dart`,
// `components/m3e_search_view.dart`, `components/m3e_search_view_build.dart`,
// `components/m3e_search_view_route.dart`,
// `styles/m3e_search_view_theme.dart`) — MIT, © 2026 Paa Developments; see
// [`super`]'s header for the family's full source list and porting decisions.

//! The M3 Expressive **search view**: an edit-field header over a
//! caller-supplied suggestion slot, presented full-screen on compact widths and
//! as an anchored panel on expanded ones.
//!
//! # One content, two presentations
//!
//! [`search_view`] returns a *builder* ([`SearchView`]) that terminates in one
//! of two hosts — see [`super`]'s presentation split for why the choice cannot
//! live inside a widget's `layout`:
//!
//! - [`SearchView::full_screen`] wraps [`crate::overlay::modal`]'s host as a
//!   full-area panel; [`show_search_view`] pushes it as a transparent navigator
//!   page, so it carries the host's staged exit ramp, its
//!   [`frust::BackPolicy::DismissAnimated`] back seam, and its `Escape`
//!   handling wholesale.
//! - [`SearchView::docked`] wraps [`crate::overlay::anchored_overlay`], placed
//!   over the bar's own rect, light-dismissing on a press outside it. The app
//!   keeps it mounted and toggles [`DockedSearchViewView::open`] — the
//!   kept-mounted pattern that host's exit ramp needs.
//!
//! Both build the same [`SearchViewContentWidget`], which reads its mode for
//! the header band and (docked only) paints the panel chrome the anchored host
//! deliberately has none of.
//!
//! # Header
//!
//! `[back][field][clear]`, with the **field spanning the whole header band**:
//! the framework's baseline editable paints its own opaque background, so
//! letting it span the band makes that fill read as one uniform field band
//! above the divider instead of a patch behind the glyphs (see [`super`]'s
//! *Fidelity decisions*). The back and clear affordances are laid out — and
//! painted — *over* it, each centred in its own [`super::ACTION_SLOT`] (48dp)
//! touch target, and the field's own left text inset clears the back slot.
//!
//! The back affordance fires [`SearchView::on_back`] **directly**, the same
//! shape [`crate::dialog`]'s header close affordance takes: the host's staged
//! dismiss seam (the back-press generation cell) is installed by
//! [`crate::overlay::show_overlay_modal`] and is not reachable from panel
//! content, so a header dismiss pops immediately while the system-back, Escape
//! and scrim paths still ramp out. [`show_search_view`] wires `on_back` to
//! `controller.pop()` for you; a docked view wires it to the same flag its
//! `on_dismiss` clears.
//!
//! # Query round trip
//!
//! The view is controlled end to end: it renders `query`, reports every edit
//! through `on_query_changed`, and the app's next rebuild feeds both the new
//! query *and* the suggestions it produced back down. Nothing is cached here —
//! the reference's own debounced `suggestionsBuilder` (a `Timer(Duration.zero)`
//! coalescing controller notifications, `m3e_search_view.dart`'s
//! `_scheduleSuggestions`) has no analogue in a synchronous rebuild model,
//! where the suggestion list is simply whatever the app hands down this frame.

use std::rc::Rc;

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CornerRadii, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, View, Widget, any,
    build_child, rebuild_child, rebuild_children, route_event, route_event_single, teardown_child,
    visit_children,
};
use frust::{NavigatorController, PopResult, Theme, icon, text_input};
use kurbo::{Point, Size};

use super::{
    ACTION_SLOT, SEARCH_VIEW_HEIGHT_FRACTION, SEARCH_VIEW_MIN_HEIGHT, SEARCH_VIEW_MIN_WIDTH,
    SearchViewMode,
};
use crate::icon_button::icon_button;
use crate::overlay::anchored::{AnchoredOverlayView, AnchoredOverlayWidget};
use crate::overlay::{
    OverlayAlign, OverlayAnchor, OverlayContainer, OverlayCorners, OverlayElevation,
    OverlayEntrance, OverlayExtent, OverlayLimit, OverlayModalConfig, OverlayModalContent,
    OverlayModalView, OverlayModalWidget, OverlayPlacement, OverlaySide, anchored_overlay,
    container, finite_or_zero, overlay_modal, radius, shadow, show_overlay_modal,
};

/// The back affordance's accessible name — `M3ESearchConstants.backButtonTooltip`.
const BACK_LABEL: &str = "Back";

/// The clear affordance's accessible name —
/// `M3ESearchConstants.clearButtonTooltip`.
const CLEAR_LABEL: &str = "Clear";

/// A view-held, typed text callback.
type OnText<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held, typed state callback.
type OnState<State> = Rc<dyn Fn(&mut State)>;

/// The M3 full-screen search-view chrome: a full-area `surfaceContainerHigh`
/// panel with square corners and no shadow (a panel filling the window has no
/// edge to lift off), sliding down from the top edge.
///
/// The scrim the host paints under it is fully occluded by the panel — the
/// reference's route is `barrierColor: transparent` for the same reason.
///
/// The entrance is [`OverlayEntrance::Slide`], the pairing
/// [`OverlayModalConfig::edge`] itself ships: an edge panel's origin is a
/// function of the ramp's progress, and only `Slide` asks for the per-frame
/// **relayout** that moves it — a fade-only entrance would composite the panel
/// at its still-off-screen progress-`0` geometry. Its 500ms
/// ([`crate::tokens::MaterialMotion::LONG_2`]) is the host's own timing, close
/// to the reference's 600ms `M3ESearchConstants.openViewDuration`.
fn full_screen_config() -> OverlayModalConfig {
    OverlayModalConfig::edge(OverlaySide::Top)
        .extent(OverlayExtent::Fraction(1.0), OverlayLimit::None)
        .corners(OverlayCorners::None)
        .container(OverlayContainer::High)
        .elevation(OverlayElevation::None)
        .entrance(OverlayEntrance::Slide)
}

/// The docked panel's placement: pinned over the anchor's own rect (the
/// reference's `_rectTween.end` topLeft *is* `anchorRect.topLeft`), never
/// flipped, shifted back on-screen when it overflows — which is the
/// reference's own `_updateTweens` fit-up step.
///
/// Expressed as a `Bottom` placement offset back up by the anchor's height,
/// since [`crate::overlay::place_anchored`] positions relative to an anchor
/// *edge*. The anchor cell is written by the trigger's own paint, so the
/// height read here is last frame's — which is exact by the time a view opens,
/// since the bar it opens from has necessarily painted first.
fn docked_placement(anchor: &OverlayAnchor) -> OverlayPlacement {
    OverlayPlacement::on(OverlaySide::Bottom)
        .align(OverlayAlign::Start)
        .offset(-anchor.rect().height())
        .flip(false)
        .clamp(true)
}

/// A search view's content, before a presentation is chosen. See the [module
/// docs](self).
pub struct SearchView<State: 'static> {
    query: String,
    on_query_changed: OnText<State>,
    hint: Option<String>,
    leading: Option<AnyView<State>>,
    trailing: Vec<AnyView<State>>,
    suggestions: Option<AnyView<State>>,
    on_submit: Option<OnText<State>>,
    on_back: Option<OnState<State>>,
    dismissable: bool,
    on_dismiss: Option<OnState<State>>,
}

/// Build a controlled M3 search view showing `query` and reporting every edit
/// through `on_query_changed(state, new_query)`.
///
/// Terminate it with [`SearchView::full_screen`] or [`SearchView::docked`] —
/// or hand it straight to [`show_search_view`], which applies the full-screen
/// presentation and wires the back affordance for you:
///
/// ```ignore
/// show_search_view(
///     &state.nav,
///     move || {
///         search_view(query.clone(), |s: &mut App, q| s.query = q)
///             .hint("Search recipes")
///             .suggestions(suggestion_list())
///     },
///     |_state: &mut App, _result: PopResult| {},
/// );
/// ```
pub fn search_view<State: 'static, F: Fn(&mut State, String) + 'static>(
    query: impl Into<String>,
    on_query_changed: F,
) -> SearchView<State> {
    SearchView {
        query: query.into(),
        on_query_changed: Rc::new(on_query_changed),
        hint: None,
        leading: None,
        trailing: Vec::new(),
        suggestions: None,
        on_submit: None,
        on_back: None,
        dismissable: true,
        on_dismiss: None,
    }
}

impl<State: 'static> SearchView<State> {
    /// Set the field's placeholder (`M3ESearchAnchor.viewHintText`), which also
    /// labels the view's accessibility node.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Replace the header's default back affordance
    /// (`M3ESearchAnchor.viewLeading`). The slot is never empty.
    pub fn leading(mut self, view: impl View<State>) -> Self {
        self.leading = Some(AnyView::new(view));
        self
    }

    /// Append a trailing header action. Setting any replaces the built-in clear
    /// button — `M3ESearchAnchor.viewTrailing`'s own
    /// `widget.viewTrailing ?? [clear]` precedence.
    pub fn trailing(mut self, view: impl View<State>) -> Self {
        self.trailing.push(AnyView::new(view));
        self
    }

    /// Set the body slot: the suggestion list this view shows under its header
    /// (`M3ESearchAnchor.suggestionsBuilder`'s output). A view with none shows
    /// an empty body.
    pub fn suggestions<V: View<State>>(mut self, view: V) -> Self {
        self.suggestions = Some(any(view));
        self
    }

    /// Set the Enter handler (`M3ESearchAnchor.viewOnSubmitted`).
    pub fn on_submit<F: Fn(&mut State, String) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// Set the header back affordance's handler. [`show_search_view`] wires
    /// this to `controller.pop()`; a docked view falls back to its own
    /// [`on_dismiss`](Self::on_dismiss) when this is unset, since a docked
    /// back press *is* a dismissal. See the [module docs](self)' header note
    /// for why this dismissal is not staged.
    pub fn on_back<F: Fn(&mut State) + 'static>(mut self, on_back: F) -> Self {
        self.on_back = Some(Rc::new(on_back));
        self
    }

    /// Whether the user can dismiss a full-screen view at all — its scrim tap,
    /// `Escape` and the Android back press (default `true`; see
    /// [`mod@crate::overlay::modal`]). No effect on a docked view, whose
    /// lifetime is the app's own `open` flag.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the dismiss callback.
    ///
    /// - **Docked**: the host's light dismiss (a press outside the panel, or
    ///   `Escape` while it holds focus) *and*, unless
    ///   [`on_back`](Self::on_back) overrides it, the header's back affordance
    ///   — wire it to the flag [`DockedSearchViewView::open`] reads.
    /// - **Full-screen**: the **unstaged** dismissal, delivered with
    ///   `&mut State` during the event pass. [`show_search_view`] wires the
    ///   staged (navigator-pop) path instead, so set this only for a
    ///   `Stack`-mounted view with no navigator underneath it.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.on_dismiss = Some(Rc::new(on_dismiss));
        self
    }

    /// Finish as the **full-screen** presentation: [`crate::overlay::modal`]'s
    /// host filling the whole area. [`show_search_view`] is the navigator push
    /// most callers want.
    pub fn full_screen(self) -> SearchViewView<State> {
        let label = self.hint.clone();
        let dismissable = self.dismissable;
        let on_dismiss = self.on_dismiss.clone();
        let content = self.into_content(SearchViewMode::FullScreen, None);
        let mut host = overlay_modal(content, full_screen_config()).dismissable(dismissable);
        if let Some(label) = label {
            host = host.label(label);
        }
        if let Some(on_dismiss) = on_dismiss {
            host = host.on_modal_dismiss(on_dismiss);
        }
        SearchViewView(host)
    }

    /// Finish as the **docked** presentation: an
    /// [`crate::overlay::anchored_overlay`] placed over `anchor`'s own rect
    /// (the bar's — wrap it in [`crate::overlay::overlay_anchor`]).
    ///
    /// Keep the host mounted and toggle [`DockedSearchViewView::open`] so its
    /// exit ramp can run (`docs/LIMITATIONS.md`'s
    /// `shadcn-anchored-exit-needs-kept-mounted`).
    ///
    /// [`on_dismiss`](Self::on_dismiss) serves both exits here — the light
    /// dismiss and the header's back affordance — so one closure closes the
    /// view however the user asks.
    ///
    /// ```ignore
    /// search_view(state.query.clone(), |s: &mut App, q| s.query = q)
    ///     .suggestions(suggestion_list())
    ///     .on_dismiss(|s: &mut App| s.search_open = false)
    ///     .docked(&state.search_anchor)
    ///     .open(state.search_open)
    /// ```
    pub fn docked(mut self, anchor: &OverlayAnchor) -> DockedSearchViewView<State> {
        let placement = docked_placement(anchor);
        let on_dismiss = self.on_dismiss.clone();
        // A docked back press *is* a dismissal, so one closure covers both
        // unless the caller set an explicit `on_back`.
        if self.on_back.is_none() {
            self.on_back = on_dismiss.clone();
        }
        let content = self.into_content(SearchViewMode::Docked, Some(anchor.clone()));
        let mut host = anchored_overlay(content)
            .anchor(anchor)
            .placement(placement);
        if let Some(on_dismiss) = on_dismiss {
            host = host.on_dismiss(move |state: &mut State| on_dismiss(state));
        }
        DockedSearchViewView(host)
    }

    /// Erase this builder into the shared content view for `mode`.
    fn into_content(
        self,
        mode: SearchViewMode,
        anchor: Option<OverlayAnchor>,
    ) -> SearchViewContentView<State> {
        SearchViewContentView {
            mode,
            anchor,
            query: self.query,
            on_query_changed: self.on_query_changed,
            hint: self.hint,
            leading: self.leading,
            trailing: self.trailing,
            suggestions: self.suggestions,
            on_submit: self.on_submit,
            on_back: self.on_back,
        }
    }
}

/// The shared header/divider/suggestions content both presentations mount.
struct SearchViewContentView<State: 'static> {
    mode: SearchViewMode,
    /// Docked only: the width source (`clamp(anchor.width, …)`).
    anchor: Option<OverlayAnchor>,
    query: String,
    on_query_changed: OnText<State>,
    hint: Option<String>,
    leading: Option<AnyView<State>>,
    trailing: Vec<AnyView<State>>,
    suggestions: Option<AnyView<State>>,
    on_submit: Option<OnText<State>>,
    on_back: Option<OnState<State>>,
}

impl<State: 'static> SearchViewContentView<State> {
    /// The header's live editable: the framework's baseline field with its own
    /// border and focus ring suppressed, spanning the header band, its glyphs
    /// inset clear of the back slot.
    fn field_view(&self) -> AnyView<State> {
        let on_change = self.on_query_changed.clone();
        let (hpad, _) = self.mode.header_padding();
        let mut field = text_input(self.query.clone(), move |state: &mut State, text| {
            on_change(state, text)
        })
        .border_width(0.0)
        .focus_ring_width(0.0)
        .corner_radius(0.0)
        .padding(hpad + ACTION_SLOT, 0.0);
        if let Some(hint) = &self.hint {
            field = field.placeholder(hint.clone());
        }
        if let Some(on_submit) = self.on_submit.clone() {
            field = field.on_submit(move |state: &mut State, text| on_submit(state, text));
        }
        any(field)
    }

    /// The header's default leading view: the M3 `arrow_back` affordance.
    fn default_leading(&self) -> AnyView<State> {
        let on_back = self.on_back.clone();
        any::<State, _>(
            icon_button(
                any::<State, _>(icon(crate::icons::ARROW_BACK)),
                move |state: &mut State| {
                    if let Some(on_back) = &on_back {
                        on_back(state);
                    }
                },
            )
            .semantic_label(BACK_LABEL),
        )
    }

    /// The leading view this pass mounts: the caller's, or `default`.
    fn leading_ref<'a>(&'a self, default: &'a AnyView<State>) -> &'a AnyView<State> {
        self.leading.as_ref().unwrap_or(default)
    }

    /// The built-in trailing actions: one clear button while the query is
    /// non-empty and the caller set no trailing of its own.
    fn generated_trailing(&self) -> Vec<AnyView<State>> {
        if !self.trailing.is_empty() || self.query.is_empty() {
            return Vec::new();
        }
        let on_query_changed = self.on_query_changed.clone();
        vec![any::<State, _>(
            icon_button(any::<State, _>(icon(crate::icons::CLOSE)), move |state| {
                on_query_changed(state, String::new());
            })
            .semantic_label(CLEAR_LABEL),
        )]
    }

    /// Every trailing view this pass mounts: the caller's own, or `generated`.
    fn trailing_refs<'a>(&'a self, generated: &'a [AnyView<State>]) -> Vec<&'a AnyView<State>> {
        if self.trailing.is_empty() {
            generated.iter().collect()
        } else {
            self.trailing.iter().collect()
        }
    }

    /// The body slot's stand-in for a view with no suggestions set: an empty
    /// text run, which fills the tight body box and paints nothing. (Not a
    /// zero-thickness divider — a divider fills whatever box it is given.)
    fn body_view(&self) -> AnyView<State> {
        any::<State, _>(frust::text(String::new()))
    }

    /// The header/body separator.
    fn divider_view(&self) -> AnyView<State> {
        any::<State, _>(crate::divider::divider())
    }
}

impl<State: 'static> View<State> for SearchViewContentView<State> {
    type Element = SearchViewContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SearchViewContentWidget {
        let default_leading = self.default_leading();
        let generated = self.generated_trailing();
        let body = self.body_view();
        SearchViewContentWidget {
            mode: self.mode,
            anchor: self.anchor.clone(),
            label: self.hint.clone(),
            field: build_child(&self.field_view(), ctx),
            leading: build_child(self.leading_ref(&default_leading), ctx),
            trailing: self
                .trailing_refs(&generated)
                .into_iter()
                .map(|v| build_child(v, ctx))
                .collect(),
            divider: build_child(&self.divider_view(), ctx),
            body: build_child(self.suggestions.as_ref().unwrap_or(&body), ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SearchViewContentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.mode = self.mode;
        element.anchor = self.anchor.clone();
        element.label = self.hint.clone();

        let mut flags = rebuild_child(
            &prev.field_view(),
            &self.field_view(),
            &mut element.field,
            ctx,
        );

        let prev_default_leading = prev.default_leading();
        let next_default_leading = self.default_leading();
        flags |= rebuild_child(
            prev.leading_ref(&prev_default_leading),
            self.leading_ref(&next_default_leading),
            &mut element.leading,
            ctx,
        );

        let prev_generated = prev.generated_trailing();
        let next_generated = self.generated_trailing();
        let prev_refs = prev.trailing_refs(&prev_generated);
        let next_refs = self.trailing_refs(&next_generated);
        flags |= rebuild_children(
            &prev_refs,
            &next_refs,
            &mut element.trailing,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        flags |= rebuild_child(
            &prev.divider_view(),
            &self.divider_view(),
            &mut element.divider,
            ctx,
        );

        let prev_body = prev.body_view();
        let next_body = self.body_view();
        flags |= rebuild_child(
            prev.suggestions.as_ref().unwrap_or(&prev_body),
            self.suggestions.as_ref().unwrap_or(&next_body),
            &mut element.body,
            ctx,
        );
        flags
    }

    fn teardown(&self, element: &mut SearchViewContentWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field_view(), &mut element.field, ctx);
        let default_leading = self.default_leading();
        teardown_child(
            self.leading_ref(&default_leading),
            &mut element.leading,
            ctx,
        );
        let generated = self.generated_trailing();
        for (view, pod) in self
            .trailing_refs(&generated)
            .into_iter()
            .zip(element.trailing.iter_mut())
        {
            teardown_child(view, pod, ctx);
        }
        teardown_child(&self.divider_view(), &mut element.divider, ctx);
        let body = self.body_view();
        teardown_child(
            self.suggestions.as_ref().unwrap_or(&body),
            &mut element.body,
            ctx,
        );
    }
}

/// The retained widget behind both search-view presentations: the header band,
/// the divider, and the suggestion slot — plus, docked, the panel chrome the
/// anchored host has none of. See the [module docs](self).
pub struct SearchViewContentWidget {
    mode: SearchViewMode,
    anchor: Option<OverlayAnchor>,
    /// The hint, kept for the docked presentation's own accessibility node
    /// (full-screen, the modal host carries the label instead).
    label: Option<String>,
    field: ChildPod,
    leading: ChildPod,
    trailing: Vec<ChildPod>,
    divider: ChildPod,
    body: ChildPod,
}

impl SearchViewContentWidget {
    /// The presentation this content is laid out for.
    pub fn mode(&self) -> SearchViewMode {
        self.mode
    }

    /// The header band's height, in logical px — 72dp full-screen, 56dp docked.
    pub fn header_height(&self) -> f64 {
        self.mode.header_height()
    }

    /// The panel size this content resolves for `area`: the whole area when
    /// full-screen, else the reference's own
    /// `clamp(anchorWidth, minWidth, maxWidth)` ×
    /// `clamp(area.height × 2⁄3, minHeight, maxHeight)`.
    fn panel_size(&self, area: Size) -> Size {
        if self.mode.is_full_screen() {
            return area;
        }
        let anchor_width = self.anchor.as_ref().map_or(0.0, |a| a.rect().width());
        // `.max(..).min(area)` rather than `clamp`: a viewport narrower/shorter
        // than the M3 minimum must win, and `clamp` would panic on the
        // inverted bounds that implies.
        let width = anchor_width.max(SEARCH_VIEW_MIN_WIDTH).min(area.width);
        let height = (area.height * SEARCH_VIEW_HEIGHT_FRACTION)
            .max(SEARCH_VIEW_MIN_HEIGHT)
            .min(area.height);
        Size::new(width.max(0.0), height.max(0.0))
    }
}

impl Widget for SearchViewContentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        let panel = self.panel_size(area);
        let header_h = self.mode.header_height();
        let (hpad, vpad) = self.mode.header_padding();

        // The field spans the whole band (see the module docs' Header note);
        // its own text inset clears the leading slot.
        self.field.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(panel.width, header_h)),
        );
        self.field.set_origin(Point::ZERO);

        let slot_bc =
            BoxConstraints::loose(Size::new(ACTION_SLOT, (header_h - 2.0 * vpad).max(0.0)));
        let leading_size = self.leading.layout_child(ctx, &slot_bc);
        self.leading.set_origin(Point::new(
            hpad + (ACTION_SLOT - leading_size.width) / 2.0,
            (header_h - leading_size.height) / 2.0,
        ));

        let mut trailing_x = panel.width - hpad;
        for pod in self.trailing.iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            trailing_x -= ACTION_SLOT;
            pod.set_origin(Point::new(
                trailing_x + (ACTION_SLOT - size.width) / 2.0,
                (header_h - size.height) / 2.0,
            ));
        }

        let divider_size = self
            .divider
            .layout_child(ctx, &BoxConstraints::loose(panel));
        self.divider.set_origin(Point::new(0.0, header_h));

        let body_y = header_h + divider_size.height;
        let body_h = (panel.height - body_y).max(0.0);
        self.body
            .layout_child(ctx, &BoxConstraints::tight(Size::new(panel.width, body_h)));
        self.body.set_origin(Point::new(0.0, body_y));

        panel
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();

        // Docked: the anchored host paints no chrome at all, so the panel's
        // fill, elevation and corners are this widget's own. Full-screen: the
        // modal host already painted (and clipped to) its panel.
        let owns_chrome = !self.mode.is_full_screen();
        if owns_chrome {
            let r = radius(theme);
            let radii = CornerRadii::new(r, r, r, r);
            if let Some((blur, y_offset, color)) = shadow(theme, OverlayElevation::Level3) {
                scene.draw_shadow(
                    Point::new(origin.x, origin.y + y_offset),
                    size,
                    r,
                    blur,
                    color,
                );
            }
            scene.fill_rounded_rect_radii(
                origin,
                size,
                radii,
                container(theme, OverlayContainer::High),
            );
            scene.push_clip_rounded_radii(origin, size, radii);
        }

        self.field.paint_child(ctx, scene);
        self.leading.paint_child(ctx, scene);
        for pod in &mut self.trailing {
            pod.paint_child(ctx, scene);
        }
        self.divider.paint_child(ctx, scene);
        self.body.paint_child(ctx, scene);

        if owns_chrome {
            scene.pop_clip();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.field.event_child(ctx, event);
            self.leading.event_child(ctx, event);
            for pod in &mut self.trailing {
                pod.event_child(ctx, event);
            }
            self.body.event_child(ctx, event);
            return EventResult::Ignored;
        }
        // The header affordances sit *over* the band-spanning field, so they
        // get the pass first; the field takes what is left of the band, and
        // the body the rest.
        if route_event_single(&mut self.leading, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event(&mut self.trailing, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        if route_event_single(&mut self.field, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        route_event_single(&mut self.body, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let children = |ctx: &mut SemanticsCtx| {
            self.leading.semantics_child(ctx);
            self.field.semantics_child(ctx);
            for pod in &self.trailing {
                pod.semantics_child(ctx);
            }
            self.divider.semantics_child(ctx);
            self.body.semantics_child(ctx);
        };
        if self.mode.is_full_screen() {
            // The modal host contributes the labelled `Role::Dialog` container
            // this content hangs under; a second one would nest.
            children(ctx);
            return;
        }
        // Docked, nothing above contributes a node — the anchored host reports
        // none of its own — so this content carries the container. It is
        // deliberately **not** flagged modal: the page underneath a docked
        // panel stays interactive.
        let label = self.label.clone();
        ctx.push_container(
            Role::Dialog,
            |node| {
                if let Some(label) = &label {
                    node.set_label(label.as_str());
                }
            },
            children,
        );
    }

    visit_children!(field, leading, trailing, divider, body);
}

/// A declarative full-screen M3 search view — [`crate::overlay::modal`]'s host,
/// pre-configured. Build one with [`SearchView::full_screen`]; push it with
/// [`show_search_view`].
pub struct SearchViewView<State: 'static>(OverlayModalView<State>);

impl<State: 'static> View<State> for SearchViewView<State> {
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

impl<State: 'static> OverlayModalContent<State> for SearchViewView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.0 = self.0.on_modal_dismiss(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.0.modal_dismissable()
    }
}

/// A declarative docked M3 search view — [`crate::overlay::anchored_overlay`]'s
/// host, pre-configured and placed over its anchor. Build one with
/// [`SearchView::docked`].
pub struct DockedSearchViewView<State: 'static>(AnchoredOverlayView<State>);

impl<State: 'static> DockedSearchViewView<State> {
    /// Tell the kept-mounted host whether it is open (default `true`). Keep the
    /// host mounted and toggle this so its exit ramp can run — see
    /// [`SearchView::docked`].
    pub fn open(mut self, open: bool) -> Self {
        self.0 = self.0.open(open);
        self
    }

    /// Replace the host's light-dismiss callback (a press outside the panel, or
    /// `Escape` once it holds focus) after the fact.
    ///
    /// [`SearchView::on_dismiss`] already wires this *and* the header's back
    /// affordance; reach for this only to give the light dismiss its own
    /// behaviour, since it leaves the back affordance on the earlier closure.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.0 = self.0.on_dismiss(on_dismiss);
        self
    }
}

impl<State: 'static> View<State> for DockedSearchViewView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.0, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.0, &prev.0, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.0, element, ctx);
    }
}

/// Push `build`'s search view as a **full-screen** transparent navigator page
/// and register `on_result` for the value it pops with — the
/// [`crate::overlay::modal`] host's shared
/// [`show_overlay_modal`](crate::overlay::show_overlay_modal) push.
///
/// The scrim tap, `Escape` and an Android back press are wired to
/// `controller.pop()` for you, staged behind the host's exit ramp; the header's
/// own back affordance is wired to the same pop, **unstaged** (see the [module
/// docs](self)' header note).
pub fn show_search_view<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> SearchView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let back_controller = controller.clone();
    show_overlay_modal(
        controller,
        move || {
            let ctrl = back_controller.clone();
            build()
                .on_back(move |_state: &mut State| ctrl.pop())
                .full_screen()
        },
        on_result,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        Key, KeyEvent, Modifiers, PointerButton, PointerEvent, PointerPhase, any as core_any,
    };
    use frust::{Color, FrameTime, NavigatorView};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use kurbo::Rect;
    use std::any::Any;

    /// A compact (phone-shaped) window, and an expanded (desktop) one.
    const COMPACT: Size = Size::new(392.0, 800.0);
    const EXPANDED: Size = Size::new(1280.0, 800.0);

    #[derive(Default)]
    struct App {
        query: String,
        open: bool,
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        panels: Vec<(Point, Size, CornerRadii, Color)>,
        clips: Vec<(Point, Size)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.panels.push((
                origin,
                size,
                CornerRadii::new(radius, radius, radius, radius),
                color,
            ));
        }
        fn fill_rounded_rect_radii(
            &mut self,
            origin: Point,
            size: Size,
            radii: CornerRadii,
            color: Color,
        ) {
            self.panels.push((origin, size, radii, color));
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn push_clip_rounded_radii(&mut self, origin: Point, size: Size, _radii: CornerRadii) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {}
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn ft(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    fn content<State: 'static>(
        mode: SearchViewMode,
        anchor: Option<OverlayAnchor>,
        query: &str,
    ) -> SearchViewContentView<State> {
        search_view(query.to_string(), |_: &mut State, _| {})
            .hint("Search")
            .into_content(mode, anchor)
    }

    fn build_content<State: 'static>(
        view: &SearchViewContentView<State>,
    ) -> SearchViewContentWidget {
        let mut counter = 0u64;
        View::<State>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SearchViewContentWidget, area: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(area))
    }

    fn dispatch(
        w: &mut SearchViewContentWidget,
        state: &mut App,
        area: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, area);
        w.event(&mut ctx, event)
    }

    /// The centre of a laid-out pod, in the content widget's own space.
    fn centre(pod: &ChildPod) -> Point {
        let o = pod.origin();
        let s = pod.size();
        Point::new(o.x + s.width / 2.0, o.y + s.height / 2.0)
    }

    // ---- Mode branching, both constraint regimes ----

    #[test]
    fn the_full_screen_content_fills_a_compact_area() {
        let view: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "");
        let mut w = build_content(&view);
        let size = layout(&mut w, COMPACT);
        assert_eq!(
            size, COMPACT,
            "the full-screen content takes the whole area"
        );
        assert_eq!(w.header_height(), super::super::FULL_SCREEN_HEADER_HEIGHT);
        assert_eq!(w.body.origin().x, 0.0);
        assert!(
            w.body.size().height > 0.0,
            "the suggestion slot fills what the header leaves"
        );
    }

    #[test]
    fn the_docked_content_takes_the_anchor_width_and_two_thirds_of_the_height() {
        let anchor = OverlayAnchor::new();
        // A 720dp-wide bar 56dp tall, 24dp down the window.
        anchor.set(Rect::new(24.0, 24.0, 744.0, 80.0));
        let view: SearchViewContentView<()> =
            content(SearchViewMode::Docked, Some(anchor.clone()), "");
        let mut w = build_content(&view);
        let size = layout(&mut w, EXPANDED);

        assert_eq!(
            size.width, 720.0,
            "the reference clamps to the anchor width"
        );
        assert_eq!(
            size.height,
            EXPANDED.height * SEARCH_VIEW_HEIGHT_FRACTION,
            "…and to two thirds of the window height"
        );
        assert_eq!(w.header_height(), super::super::SEARCH_BAR_MIN_HEIGHT);
    }

    #[test]
    fn a_narrow_anchor_still_gets_the_reference_min_width() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, 0.0, 120.0, 56.0));
        let view: SearchViewContentView<()> = content(SearchViewMode::Docked, Some(anchor), "");
        let mut w = build_content(&view);
        let size = layout(&mut w, EXPANDED);
        assert_eq!(size.width, SEARCH_VIEW_MIN_WIDTH);
    }

    #[test]
    fn a_short_area_clamps_the_panel_to_it_rather_than_to_the_min_height() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, 0.0, 400.0, 56.0));
        let view: SearchViewContentView<()> = content(SearchViewMode::Docked, Some(anchor), "");
        let mut w = build_content(&view);
        // 200dp tall: 2/3 is 133, below the 240 minimum, which itself exceeds
        // the area — the area wins rather than overflowing.
        let size = layout(&mut w, Size::new(900.0, 200.0));
        assert_eq!(size.height, 200.0);
    }

    #[test]
    fn only_the_docked_panel_paints_its_own_chrome() {
        let theme = crate::baseline();
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, 0.0, 400.0, 56.0));

        let docked: SearchViewContentView<()> = content(SearchViewMode::Docked, Some(anchor), "");
        let mut w = build_content(&docked);
        let size = layout(&mut w, EXPANDED);
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, ft(0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);
        let panel = rec
            .panels
            .first()
            .expect("the docked panel paints its own container");
        assert_eq!(panel.3, theme.scheme().surface_container_high);
        assert_eq!(panel.2.top_left, theme.shape.extra_large);
        assert!(!rec.clips.is_empty(), "…and clips its content to it");

        let full: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "");
        let mut w = build_content(&full);
        let size = layout(&mut w, COMPACT);
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, ft(0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);
        assert!(
            rec.clips.is_empty(),
            "the modal host already clipped the full-screen panel"
        );
    }

    #[test]
    fn a_view_with_no_suggestions_paints_nothing_in_its_body() {
        let view: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "");
        let mut w = build_content(&view);
        let size = layout(&mut w, COMPACT);
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, size, ft(0));
        w.paint(&mut pctx, &mut rec);

        let header_h = w.header_height();
        for (origin, painted, _) in &rec.rects {
            assert!(
                origin.y + painted.height <= header_h + 2.0,
                "only the header band and the hairline divider fill a rect; \
                 found one at {origin:?} sized {painted:?}"
            );
        }
    }

    // ---- Header composition ----

    #[test]
    fn the_header_puts_the_back_slot_over_the_band_spanning_field() {
        let view: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "");
        let mut w = build_content(&view);
        layout(&mut w, COMPACT);

        assert_eq!(w.field.origin(), Point::ZERO);
        assert_eq!(
            w.field.size(),
            Size::new(COMPACT.width, super::super::FULL_SCREEN_HEADER_HEIGHT),
            "the field spans the whole header band"
        );
        let (hpad, _) = SearchViewMode::FullScreen.header_padding();
        assert!(
            w.leading.origin().x >= hpad,
            "the back affordance sits inside the header inset"
        );
        assert!(
            w.leading.origin().x < hpad + ACTION_SLOT,
            "…centred in its own 48dp slot"
        );
    }

    #[test]
    fn a_non_empty_query_grows_the_clear_affordance() {
        let empty: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "");
        assert!(build_content(&empty).trailing.is_empty());

        let filled: SearchViewContentView<()> = content(SearchViewMode::FullScreen, None, "abc");
        let mut w = build_content(&filled);
        assert_eq!(w.trailing.len(), 1);
        layout(&mut w, COMPACT);
        assert!(
            w.trailing[0].origin().x > COMPACT.width - 2.0 * ACTION_SLOT,
            "the clear affordance packs against the end edge"
        );
    }

    // ---- Query round trip ----

    #[test]
    fn typing_reports_through_on_query_changed_and_rebuilds_the_suggestions() {
        /// One suggestion row per query character — so the assertion below
        /// proves the *round trip*, not just the report.
        fn logic(state: &mut App) -> SearchViewContentView<App> {
            let rows = state.query.chars().count().max(1) as f64;
            search_view(state.query.clone(), |s: &mut App, q| s.query = q)
                .hint("Search")
                .suggestions(leaf_any_app(100.0, 24.0 * rows))
                .into_content(SearchViewMode::FullScreen, None)
        }
        let mut root: RenderRoot<App, SearchViewContentView<App>> = RenderRoot::new();
        let mut state = App::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);

        // Focus the field (a press in the band, clear of the back slot), then
        // type: the baseline editable takes the keystrokes and reports them.
        root.event(&mut state, &ev(PointerPhase::Down, 200.0, 36.0));
        root.event(&mut state, &ev(PointerPhase::Up, 200.0, 36.0));
        root.event(&mut state, &key('h'));
        root.event(&mut state, &key('i'));
        assert_eq!(
            state.query, "hi",
            "the header field reports through on_query_changed"
        );

        // The app's own rebuild feeds the new query — and the suggestions it
        // produced from it — back down.
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);
    }

    /// A character keystroke, the shape the baseline editable inserts on.
    fn key(ch: char) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(ch.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    // ---- Full-screen presentation: staged exit through the navigator ----

    #[derive(Default)]
    struct NavState {
        query: String,
        results: Vec<Option<i32>>,
    }

    #[test]
    fn show_search_view_stages_a_back_request_and_pops() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || {
                    core_any::<NavState, _>(BgPage {
                        size: Size::new(COMPACT.width, COMPACT.height),
                    })
                })
            }
        };
        let mut state = NavState::default();
        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);

        show_search_view(
            &controller,
            || search_view(String::new(), |s: &mut NavState, q| s.query = q).hint("Search"),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);
        for t in [0u64, 100, 250] {
            root.paint(&mut Recorder::default(), ft(t));
        }

        assert!(
            controller.back_interest(),
            "a dismissable search view claims back interest"
        );
        controller.request_back();
        root.rebuild(&mut app, &mut state);
        assert_eq!(
            controller.depth(),
            2,
            "DismissAnimated: the page is still up while the exit ramps"
        );

        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);
            let outcome = root.paint(&mut Recorder::default(), ft(300 + frames * 100));
            frames += 1;
            assert!(frames < 30, "the staged exit settles in bounded frames");
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));
        assert_eq!(state.results, vec![None], "the back press dismissed it");
        assert_eq!(controller.depth(), 1);
    }

    #[test]
    fn the_full_screen_host_panel_covers_the_whole_area_once_settled() {
        let view: SearchViewView<App> = search_view(String::new(), |s: &mut App, q| s.query = q)
            .hint("Search")
            .suggestions(leaf_any_app(100.0, 100.0))
            .full_screen();
        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(COMPACT));
        assert_eq!(size, COMPACT, "the host fills its own area");
        assert_eq!(
            w.progress(),
            0.0,
            "the slide entrance starts off its own (top) edge"
        );

        // Settle the ramp: one paint seeds it, one well past its duration ends
        // it, each followed by the relayout the slide asks for.
        for t in [0u64, 800] {
            let mut pctx = PaintCtx::for_test(Point::ZERO, COMPACT, ft(t));
            w.paint(&mut pctx, &mut Recorder::default());
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            w.layout(&mut lctx, &BoxConstraints::tight(COMPACT));
        }

        assert_eq!(w.progress(), 1.0);
        assert_eq!(
            w.panel_rect(),
            Rect::from_origin_size(Point::ZERO, COMPACT),
            "the settled full-screen panel is the whole area — no scrim shows"
        );
        assert_eq!(
            full_screen_config().corners,
            OverlayCorners::None,
            "a panel meeting all four screen edges rounds nothing"
        );
    }

    #[test]
    fn a_non_dismissable_full_screen_view_refuses_the_back_press() {
        let view: SearchViewView<()> = search_view(String::new(), |_: &mut (), _| {})
            .dismissable(false)
            .full_screen();
        assert!(!OverlayModalContent::modal_dismissable(&view));
    }

    // ---- Docked presentation: placement over the anchor ----

    #[test]
    fn the_docked_panel_is_placed_over_the_anchors_own_rect() {
        let anchor = OverlayAnchor::new();
        let bar = Rect::new(24.0, 24.0, 744.0, 80.0);
        anchor.set(bar);

        let placement = docked_placement(&anchor);
        assert_eq!(placement.side, OverlaySide::Bottom);
        assert_eq!(placement.align, OverlayAlign::Start);
        assert_eq!(
            placement.offset,
            -bar.height(),
            "offset back up by the anchor's height so the panel covers it"
        );
        assert!(!placement.flip, "a covering panel never flips sides");

        let placed = crate::overlay::place_anchored(
            bar,
            Size::new(720.0, 533.0),
            Rect::from_origin_size(Point::ZERO, EXPANDED),
            placement,
        );
        assert_eq!(
            placed.origin(),
            bar.origin(),
            "the panel's top-left is the anchor's"
        );
    }

    #[test]
    fn a_docked_view_lays_out_and_paints_through_its_anchored_host() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(24.0, 24.0, 744.0, 80.0));
        let view: DockedSearchViewView<App> =
            search_view(String::new(), |s: &mut App, q| s.query = q)
                .hint("Search")
                .suggestions(leaf_any_app(100.0, 100.0))
                .docked(&anchor)
                .open(true)
                .on_dismiss(|s: &mut App| s.open = false);

        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(EXPANDED));
        assert_eq!(size, EXPANDED, "the host fills its area");
        assert_eq!(
            w.content_rect().origin(),
            Point::new(24.0, 24.0),
            "the placed panel covers the anchor"
        );
    }

    #[test]
    fn a_docked_views_on_dismiss_also_serves_its_header_back_affordance() {
        let anchor = OverlayAnchor::new();
        let bar = Rect::new(24.0, 24.0, 744.0, 80.0);
        anchor.set(bar);
        let view: DockedSearchViewView<App> =
            search_view(String::new(), |s: &mut App, q| s.query = q)
                .suggestions(leaf_any_app(100.0, 100.0))
                .on_dismiss(|s: &mut App| s.open = false)
                .docked(&anchor);

        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(EXPANDED));
        let mut pctx = PaintCtx::for_test(Point::ZERO, EXPANDED, ft(0));
        w.paint(&mut pctx, &mut Recorder::default());

        // The back affordance's own 48dp slot: one 8dp docked header inset in
        // from the panel's leading edge, vertically centred in the 56dp band.
        let (hpad, _) = SearchViewMode::Docked.header_padding();
        let cx = bar.x0 + hpad + ACTION_SLOT / 2.0;
        let cy = bar.y0 + SearchViewMode::Docked.header_height() / 2.0;

        let mut state = App {
            open: true,
            ..App::default()
        };
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let state_any: &mut dyn Any = &mut state;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, EXPANDED);
            w.event(&mut ctx, &ev(phase, cx, cy));
        }
        assert!(
            !state.open,
            "one `on_dismiss` closure covers the light dismiss and the back \
             affordance both"
        );
    }

    #[test]
    fn a_press_outside_the_docked_panel_dismisses_it() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(24.0, 24.0, 744.0, 80.0));
        let view: DockedSearchViewView<App> =
            search_view(String::new(), |s: &mut App, q| s.query = q)
                .suggestions(leaf_any_app(100.0, 100.0))
                .on_dismiss(|s: &mut App| s.open = false)
                .docked(&anchor)
                .open(true);

        let mut counter = 0u64;
        let mut w = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(EXPANDED));
        let mut pctx = PaintCtx::for_test(Point::ZERO, EXPANDED, ft(0));
        w.paint(&mut pctx, &mut Recorder::default());

        let mut state = App {
            open: true,
            ..App::default()
        };
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, EXPANDED);
        // The bottom-left corner, well clear of the placed panel.
        w.event(&mut ctx, &ev(PointerPhase::Down, 8.0, 780.0));
        assert!(!state.open, "a light dismiss clears the app's own flag");
    }

    #[test]
    fn the_header_back_affordance_fires_on_back() {
        let view: SearchViewContentView<App> = search_view(String::new(), |_: &mut App, _| {})
            .on_back(|s: &mut App| s.open = false)
            .into_content(SearchViewMode::FullScreen, None);
        let mut w = build_content(&view);
        layout(&mut w, COMPACT);

        let mut state = App {
            open: true,
            ..App::default()
        };
        let c = centre(&w.leading);
        dispatch(
            &mut w,
            &mut state,
            COMPACT,
            &ev(PointerPhase::Down, c.x, c.y),
        );
        dispatch(&mut w, &mut state, COMPACT, &ev(PointerPhase::Up, c.x, c.y));
        assert!(!state.open, "the back affordance fired on_back");
    }

    // ---- Semantics ----

    #[test]
    fn a_docked_view_carries_its_own_labelled_non_modal_dialog_node() {
        fn logic(_s: &mut ()) -> SearchViewContentView<()> {
            let anchor = OverlayAnchor::new();
            anchor.set(Rect::new(0.0, 0.0, 400.0, 56.0));
            search_view(String::new(), |_: &mut (), _| {})
                .hint("Search recipes")
                .into_content(SearchViewMode::Docked, Some(anchor))
        }
        let mut root: RenderRoot<(), SearchViewContentView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(EXPANDED, &mut tcx as &mut dyn Any);

        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("the docked content contributes the container node itself");
        assert_eq!(node.label(), Some("Search recipes"));
        assert!(
            !node.is_modal(),
            "the page under a docked panel stays interactive"
        );
    }

    #[test]
    fn a_full_screen_view_leaves_the_container_node_to_its_host() {
        fn logic(_s: &mut ()) -> SearchViewContentView<()> {
            search_view(String::new(), |_: &mut (), _| {})
                .hint("Search recipes")
                .into_content(SearchViewMode::FullScreen, None)
        }
        let mut root: RenderRoot<(), SearchViewContentView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(COMPACT, &mut tcx as &mut dyn Any);

        let update = root.semantics();
        assert!(
            !update.nodes.iter().any(|(_, n)| n.role() == Role::Dialog),
            "a second Dialog node would nest inside the modal host's own"
        );
    }

    // ---- Fixtures ----

    fn leaf_any_app(w: f64, h: f64) -> AnyView<App> {
        core_any::<App, _>(FixedLeaf {
            size: Size::new(w, h),
        })
    }
    struct FixedLeaf {
        size: Size,
    }
    struct FixedLeafW {
        size: Size,
    }
    impl View<App> for FixedLeaf {
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
}
