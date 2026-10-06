// Ported from `material_3_expressive` v1.0.8's `M3ENavigationDrawer` family
// (MIT, © 2026 Paa Developments;
// `tmp/material_3_expressive/lib/components/navigation_drawer/`:
// `m3e_navigation_drawer.dart`, `models/m3e_navigation_destination.dart`,
// `styles/m3e_navigation_drawer_theme.dart`,
// `components/m3e_drawer_destination_button.dart`; plus the shared
// `lib/components/navigation_rail/components/m3e_nav_selection_indicator.dart`
// the drawer imports directly from the rail package, retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
//
// Porting decisions: the modal presentation (scrim, edge slide, staged
// dismissal) is **entirely this port's own addition**, not upstream's —
// `M3ENavigationDrawer` itself is bare content (a fixed-width, full-height
// `Container` plus a destination `Column`) with no scrim, no `.show()`, and
// no dismiss wiring of any kind; Flutter's own `Scaffold` supplies all of
// that when the widget rides `Scaffold.drawer`. This port fills the same gap
// through the crate's own `overlay::modal` host instead (the `side_sheet`
// precedent, mirrored onto the opposite edge) and additionally exposes the
// bare content directly, so both of upstream's implicit usages — hosted
// modally, or embedded as a permanent layout-participating rail-like panel —
// have a real entry point here. See the module docs below for the rest.

//! The Material 3 Expressive **navigation drawer**: a fixed-width destination
//! list sliding in from the leading (left) screen edge, sharing its liquid
//! selection indicator with [`mod@crate::navigation_rail`].
//!
//! # Two entry points, one content widget
//!
//! - [`navigation_drawer_content`] returns [`DrawerContentView`] — the bare
//!   destination list upstream actually ships (fixed [`DRAWER_WIDTH`],
//!   whatever height it's given, no scrim, no dismissal): a **standard,
//!   non-modal, layout-participating** widget a caller can drop straight into
//!   a [`frust::Row`] beside page content, e.g. a permanent wide-screen
//!   drawer. This *is* upstream's own `M3ENavigationDrawer` shape — see the
//!   Porting decisions above.
//! - [`navigation_drawer`] wraps the same content in this crate's
//!   [`crate::overlay::modal`] host, configured via
//!   [`OverlayModalConfig::edge`]`(`[`OverlaySide::Left`]`)`: the scrim, the
//!   edge-pinned slide-in/out, the staged exit ramp, the
//!   [`frust::BackPolicy::DismissAnimated`] back-press seam, and the
//!   scrim-tap dismiss are every one of them the host's, reused wholesale —
//!   the exact [`mod@crate::side_sheet`] precedent, mirrored onto the
//!   opposite edge. [`show_navigation_drawer`] is the navigator-pushed
//!   convenience over it, the same shape as [`crate::side_sheet::show_side_sheet`].
//!
//! # Geometry: fixed width, right corners only
//!
//! [`DRAWER_WIDTH`] (360dp, `M3ENavigationDrawerTheme`'s own default) is
//! plumbed through [`OverlayModalConfig::extent`] as
//! [`OverlayExtent::Content`] capped at [`OverlayLimit::Px`]`(DRAWER_WIDTH)` —
//! the [`mod@crate::side_sheet`] shape, capped down only when the viewport
//! itself is narrower. [`OverlayModalConfig::edge`]'s own `OverlaySide::Left`
//! mapping already rounds only the panel's *trailing* (right, screen-facing)
//! corners ([`OverlayCorners::End`]); the left edge itself sits flush against
//! the screen and stays square. The standalone [`DrawerContentWidget`] self-
//! sizes to the same [`DRAWER_WIDTH`] preference (clamped into whatever its
//! own constraints allow) so both entry points share one width rule.
//!
//! # The liquid selection indicator: transcribed, not reused
//!
//! Upstream's drawer imports `M3ENavSelectionIndicator` **directly from the
//! rail package** — the exact same shared-overlay, two-spring mechanism
//! [`mod@crate::navigation_rail`] already ported (main-axis lead/trail edge
//! centers, [`crate::navbar::LEAD_SPRING`]/[`crate::navbar::TRAIL_SPRING`]
//! stiffness/damping, a bridge stretch on selection and a stadium settle).
//! [`mod@crate::navbar`]'s own crate-visible `LiquidIndicator`/`LiquidEdge`
//! block invites exactly this reuse (see its module docs' *Reuse across the
//! nav family*) — but that invitation cannot actually be taken here: `LiquidEdge`'s
//! `position`/`velocity` fields carry no accessor beyond `is_animating`,
//! module-private to `navbar.rs`, and this task's scope holds `navbar.rs`
//! **read-only**. [`mod@crate::navigation_rail`] hit the identical wall for a
//! different reason (its own doc calls the rail's and the bar's indicators
//! "different mechanisms upstream", which is true of the *bar* — but the
//! rail's own reason not to reuse `LiquidIndicator` was that its `sync`/
//! `pill_rect` hardcode the horizontal main axis, exactly the gap
//! `LiquidEdge`'s own doc comment flags for "a vertical consumer" to close).
//! Since the drawer shares the rail's *actual* upstream mechanism (the same
//! Dart file), not the bar's, and the accessor gap is the binding constraint,
//! this module transcribes its own small [`Edge`]/[`DrawerIndicator`] pair
//! rather than re-deriving the spring math: [`Edge`] is a field-for-field copy
//! of `navbar::LiquidEdge`'s solve, and [`crate::navbar::LEAD_SPRING`]/
//! [`crate::navbar::TRAIL_SPRING`] (both crate-public consts, unlike the
//! struct fields) are imported and reused directly, so the motion stays
//! byte-for-byte identical to the bar's and the rail's — only the thin
//! main-axis↔`y` / cross-axis↔`x` mapping inside [`DrawerIndicator::sync`]/
//! [`DrawerIndicator::pill_rect`] is drawer-specific, exactly the swap
//! `LiquidEdge`'s doc predicts.
//!
//! Unlike the bar (which paints one pill per resting geometry read straight
//! out of the same layout pass — see [`mod@crate::navbar`]'s "One pill, not an
//! overlay plus a resting copy" porting note) and the rail (same
//! simplification, its own `use_local_indicator` handshake), the drawer takes
//! that shape too: [`DrawerContentWidget`] itself owns the one pill, painted
//! under the selected row's own content every frame, settled or traveling; no
//! row paints a resting fill of its own.
//!
//! # Sections, headers, dividers: a widening beyond the single-headline upstream
//!
//! `M3ENavigationDrawer` itself takes a *flat* `destinations` list plus at
//! most one optional `headline` string. This port generalizes that into
//! [`DrawerSection`] — repeatable header-plus-destinations groups, `Vec<`[`DrawerSection`]`>`
//! — the exact shape [`mod@crate::navigation_rail`]'s own `RailSection`
//! already carries for the *same* upstream widening
//! (`M3ENavigationRailSection`, `models/m3e_navigation_rail_section.dart`),
//! and for the same reason: a drawer commonly groups an app's primary and
//! secondary destinations. A single, unlabeled section reproduces upstream's
//! flat-list shape exactly (no divider painted at all — see below); a header
//! string reproduces the `headline` treatment verbatim (`titleSmall`,
//! `onSurfaceVariant`, [`HEADLINE_PAD_H`]×[`HEADLINE_PAD_V`] padding), just
//! repeatable per section. A full-width [`DIVIDER_THICKNESS`] hairline
//! (`outlineVariant`) paints above every section **after** the first — never
//! above the first, so the single-section case stays divider-free.
//!
//! # Badges: `badgeLabel` ported verbatim; `showBadge` completed, not copied inert
//!
//! `M3ENavigationDestination.badgeLabel` paints as trailing `labelLarge` text
//! at the row's end — ported as-is via [`DrawerDestination::badge_label`].
//! `M3ENavigationDestination.showBadge`, however, is a **documented upstream
//! gap**: it is a real, declared field (the reference's own playground and
//! README examples pass `showBadge: true`), but `m3e_drawer_destination_button.dart`'s
//! paint code never reads it at all — setting it upstream changes nothing on
//! screen. This port does not carry that inertness forward: [`DrawerDestination::show_badge`]
//! paints upstream's evident intent instead, a small [`BADGE_DOT`] circle at
//! the same trailing slot [`mod@crate::navigation_rail`]'s own dot badge takes
//! (`badge_label` wins if both are set). Shipping a builder knob that
//! silently does nothing is a worse defect than this small, clearly-scoped
//! divergence from an upstream field nobody's paint code actually consumes.
//!
//! # Selecting does not dismiss: no such wiring exists upstream to read
//!
//! The task brief this module was built from describes selecting a
//! destination as "typically" dismissing the drawer. Upstream carries **no**
//! dismiss wiring anywhere reachable from `M3ENavigationDrawer` to confirm
//! that against: the widget itself has no dismiss/pop concept at all (see the
//! Porting decisions above), its README snippet's `onDestinationSelected: (i)
//! => setState(() => drawerIndex = i)` calls nothing else, and the bundled
//! playground demo (`example/lib/pages/playground/nav/navigation_drawer_playground.dart`)
//! does the same. This port matches the code exactly as it stands rather than
//! an assumed convention: [`DrawerContentView`]'s `on_select` never dismisses
//! anything on its own — the same controlled, never-self-mutating contract
//! every control in this catalog follows. An app that wants the common
//! real-world dismiss-on-select UX calls `controller.pop()` (or its own
//! signal write) from inside its own `on_select`, exactly like any other
//! app-owned dismiss decision in this crate — but that pop is **unstaged**:
//! immediate, with no reverse-ramp exit, while every host-chrome dismiss
//! gesture on the same drawer (scrim tap, `Escape`, an Android back press)
//! stages one (`docs/LIMITATIONS.md`'s
//! `material-modal-staged-dismiss-private-to-host`). A user who dismisses by
//! selecting sees a different exit motion than one who taps the scrim.
//!
//! # No edge-swipe-to-open: absent upstream
//!
//! `M3ENavigationDrawer` carries no gesture recognizer of any kind — no swipe
//! detector, no drag handle, nothing resembling
//! [`crate::overlay::modal::OverlayModalConfig::drag`]. Interactive
//! edge-swipe-to-open a drawer is a *host* affordance (Flutter's own
//! `Scaffold` owns it, outside this widget's file entirely) with nothing in
//! this package to port. [`navigation_drawer`] leaves
//! [`OverlayModalConfig::drag`] at its `edge()` default (`false`); opening is
//! an app-driven [`show_navigation_drawer`] push, exactly like every other
//! modal host in this crate.
//!
//! # Semantics
//!
//! [`DrawerContentWidget`] contributes one [`Role::TabList`] container node
//! with a [`Role::Tab`] node per destination — the same vocabulary choice
//! [`mod@crate::navbar`]/[`mod@crate::navigation_rail`] make for "one of a set
//! of mutually-exclusive destinations", carried over here for family
//! consistency. Section headers and dividers contribute no node of their own
//! (pure paint chrome, like every section header in
//! [`mod@crate::navigation_rail`]). The modal wrapper's own
//! [`OverlayModalWidget::semantics`] contributes the enclosing
//! [`Role::Dialog`] node (accesskit modal flag set), labelled "Navigation
//! drawer".

use std::rc::Rc;

use frust::authoring::text::{
    FontWeight, LineHeight, TextContext, TextLayout, TextOverflow, TextStyle,
};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent,
    PointerPhase, Role, SemanticsCtx, View, Widget, build_child, rebuild_child, route_event,
    teardown_child, visit_children,
};
use frust::{FrameTime, NavigatorController, PopResult, Spring, SpringDesc, Theme};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::{Brush, Color};

use crate::overlay::{
    OverlayExtent, OverlayLimit, OverlayModalConfig, OverlayModalContent, OverlayModalView,
    OverlayModalWidget, OverlaySide, overlay_modal, show_overlay_modal,
};
use crate::press::presses;
use crate::tokens::MaterialSpacing;

/// This module's fixed panel width, in logical px — `M3ENavigationDrawerTheme`'s
/// own `width` default. See the [module docs](self)' Geometry section.
pub const DRAWER_WIDTH: f64 = 360.0;
/// A destination row's own height, in logical px (`destinationHeight`).
const DESTINATION_HEIGHT: f64 = 56.0;
/// A section headline's horizontal padding, in logical px
/// (`headlineHorizontalPadding`).
const HEADLINE_PAD_H: f64 = 28.0;
/// A section headline's vertical padding, in logical px
/// (`headlineVerticalPadding`) — applied above *and* below, matching
/// upstream's `EdgeInsets.symmetric`.
const HEADLINE_PAD_V: f64 = 16.0;
/// A destination icon's box, in logical px (`iconSize`).
const ICON_SIZE: f64 = 24.0;
/// A destination row's own horizontal inset from the drawer's edges, in
/// logical px (`destinationHorizontalPadding`) — matches [`MaterialSpacing::MD`].
const DESTINATION_PAD_H: f64 = MaterialSpacing::MD;
/// A destination row's own vertical inset, in logical px
/// (`destinationVerticalPadding`) — no token in [`MaterialSpacing`] matches
/// upstream's `2`.
const DESTINATION_PAD_V: f64 = 2.0;
/// A destination row's inner horizontal padding, in logical px
/// (`destinationInnerHorizontalPadding`) — matches [`MaterialSpacing::LG`].
const DESTINATION_INNER_PAD_H: f64 = MaterialSpacing::LG;
/// Gap between a destination's icon and its label, in logical px
/// (`iconLabelGap`) — matches [`MaterialSpacing::MD`].
const ICON_LABEL_GAP: f64 = MaterialSpacing::MD;
/// A destination row's full outer height, including its own vertical inset on
/// both edges — [`DESTINATION_HEIGHT`] + 2×[`DESTINATION_PAD_V`].
const ROW_OUTER_HEIGHT: f64 = DESTINATION_HEIGHT + 2.0 * DESTINATION_PAD_V;
/// A section divider's thickness, in logical px — matches
/// [`mod@crate::divider`]'s own `DEFAULT_THICKNESS`.
const DIVIDER_THICKNESS: f64 = 1.0;
/// A section header's own resolved row height (one `titleSmall` line plus its
/// vertical padding on both edges) — used only as the *unbounded-height*
/// layout fallback (see [`DrawerContentWidget::natural_height`]); the real
/// layout pass always re-shapes the header run against the resolved width.
const HEADER_ROW_HEIGHT: f64 = 20.0 + 2.0 * HEADLINE_PAD_V;
/// The dot badge's diameter, in logical px — matches
/// [`mod@crate::navigation_rail`]'s own `BADGE_DOT`. See the [module
/// docs](self)' Badges section for why this port paints it at all.
const BADGE_DOT: f64 = 8.0;

/// `label_large` — a destination's label and its trailing badge text
/// (`m3e_drawer_destination_button.dart`'s `theme.typeScale.labelLarge`, used
/// for both).
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
/// `title_small` — a section headline (`M3ENavigationDrawer`'s own headline
/// style, `theme.typeScale.titleSmall`).
const TITLE_SMALL: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);

/// Unthemed-fallback `on_secondary_container` — a selected row's ink
/// (`destinationForegroundColor(selected: true)`). Matches
/// [`mod@crate::icon_button`]'s own fallback of the same role.
const ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `on_surface_variant` — an unselected row's ink, and a
/// section headline's ink.
const ON_SURFACE_VARIANT: Color = Color::from_rgb8(0x49, 0x45, 0x4F);
/// Unthemed-fallback `secondary_container` — the liquid indicator's fill.
/// Matches [`mod@crate::navbar`]'s own fallback of the same role.
const SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `outline_variant` — a section divider's line color.
const OUTLINE_VARIANT: Color = Color::from_rgb8(0xCA, 0xC4, 0xD0);

// ---------------------------------------------------------------------------
// The liquid selection indicator (transcribed — see the module docs)
// ---------------------------------------------------------------------------

/// Rest threshold for a travelling edge, in logical px (and px/s velocity) —
/// matches [`crate::navbar`]'s own `PILL_REST_EPSILON`.
const PILL_REST_EPSILON: f64 = 0.01;
/// Main-axis movement below this (logical px) is not worth a travel — matches
/// [`crate::navbar`]'s own `GEOMETRY_EPSILON`.
const GEOMETRY_EPSILON: f64 = 0.5;

/// One spring-driven edge of the liquid pill, in drawer-local **main-axis**
/// (vertical) px. A field-for-field transcription of
/// [`crate::navbar::LiquidEdge`]'s solve — see the [module docs](self)' liquid
/// indicator section for why it is copied rather than imported.
#[derive(Clone, Copy, Debug)]
struct Edge {
    desc: SpringDesc,
    /// Equilibrium this edge is settling toward.
    target: f64,
    /// The in-flight solution, or `None` once settled.
    flight: Option<Spring>,
    /// Seconds since [`Self::flight`] was solved.
    elapsed: f64,
    position: f64,
    velocity: f64,
}

impl Edge {
    const fn new(desc: SpringDesc) -> Self {
        Edge {
            desc,
            target: 0.0,
            flight: None,
            elapsed: 0.0,
            position: 0.0,
            velocity: 0.0,
        }
    }

    /// Jump to `to` with no motion.
    fn snap(&mut self, to: f64) {
        self.target = to;
        self.position = to;
        self.velocity = 0.0;
        self.flight = None;
        self.elapsed = 0.0;
    }

    /// Start (or re-aim) a travel toward `to`, continuing from the current
    /// position and velocity.
    fn retarget(&mut self, to: f64) {
        self.flight = Some(Spring::new(self.desc, self.position - to, self.velocity));
        self.target = to;
        self.elapsed = 0.0;
    }

    /// Advance `dt` seconds, returning whether the edge is still travelling.
    fn advance(&mut self, dt: f64) -> bool {
        let Some(spring) = self.flight else {
            return false;
        };
        self.elapsed += dt;
        if spring.is_at_rest(self.elapsed, PILL_REST_EPSILON) {
            self.settle();
            return false;
        }
        self.position = self.target + spring.position(self.elapsed);
        self.velocity = spring.velocity(self.elapsed);
        true
    }

    /// Land on the target immediately, dropping any in-flight solution.
    fn settle(&mut self) {
        self.position = self.target;
        self.velocity = 0.0;
        self.flight = None;
        self.elapsed = 0.0;
    }

    fn is_animating(&self) -> bool {
        self.flight.is_some()
    }

    /// Whether this edge's committed target already sits within
    /// [`GEOMETRY_EPSILON`] of `to` — the reference's `_isAtGeometry` test.
    /// Distinguishes a real geometry change (needs an immediate jump) from an
    /// unrelated relayout pass reporting the *same* geometry mid-travel,
    /// which must not abort the in-flight animation.
    fn is_at_geometry(&self, to: f64) -> bool {
        (self.target - to).abs() < GEOMETRY_EPSILON
    }
}

/// The drawer's liquid selection indicator: two independently-sprung edge
/// centers on the **vertical** main axis (the drawer's own list runs
/// top-to-bottom), whose span is the painted pill — the [`crate::navbar::LiquidIndicator`]
/// shape with `x`/`y` swapped. See the [module docs](self)' liquid indicator
/// section.
struct DrawerIndicator {
    lead: Edge,
    trail: Edge,
    /// Clock for both edges; `None` re-seeds the delta on the next advance.
    last_time: Option<FrameTime>,
    /// The selected row's resting box, in drawer-local px.
    rest: Rect,
    ready: bool,
}

impl DrawerIndicator {
    fn new() -> Self {
        DrawerIndicator {
            lead: Edge::new(crate::navbar::LEAD_SPRING),
            trail: Edge::new(crate::navbar::TRAIL_SPRING),
            last_time: None,
            rest: Rect::ZERO,
            ready: false,
        }
    }

    /// Adopt the selected row's resting box. `travel` runs the two-phase
    /// motion; otherwise the pill jumps — but only if the geometry actually
    /// moved. An unrelated relayout pass that reports the *same* center with
    /// `travel: false` (selection unchanged) must not abort an in-flight
    /// travel; a genuine geometry change (first layout, or the resting box
    /// itself moving/resizing) still jumps immediately.
    fn sync(&mut self, rest: Rect, travel: bool) {
        self.rest = rest;
        let center = rest.center().y;
        if !self.ready {
            self.lead.snap(center);
            self.trail.snap(center);
            self.last_time = None;
            self.ready = true;
            return;
        }
        if !travel {
            if self.lead.is_at_geometry(center) && self.trail.is_at_geometry(center) {
                return;
            }
            self.lead.snap(center);
            self.trail.snap(center);
            self.last_time = None;
            return;
        }
        if (self.lead.target - center).abs() < GEOMETRY_EPSILON && !self.is_animating() {
            return;
        }
        self.lead.retarget(center);
        self.trail.retarget(center);
        self.last_time = None;
    }

    /// Advance both edges to frame time `now`, returning whether the pill is
    /// still travelling.
    fn advance(&mut self, now: FrameTime) -> bool {
        let dt = match self.last_time {
            Some(last) => now.saturating_sub(last).as_secs_f64(),
            None => 0.0,
        };
        self.last_time = Some(now);
        let lead = self.lead.advance(dt);
        let trail = self.trail.advance(dt);
        lead || trail
    }

    /// Land the pill on its target immediately (reduced motion).
    fn settle(&mut self) {
        self.lead.settle();
        self.trail.settle();
    }

    fn is_animating(&self) -> bool {
        self.lead.is_animating() || self.trail.is_animating()
    }

    /// The painted pill in drawer-local px, or `None` before first layout.
    fn pill_rect(&self) -> Option<Rect> {
        if !self.ready {
            return None;
        }
        let base = self.rest.height();
        let min = self.lead.position.min(self.trail.position);
        let max = self.lead.position.max(self.trail.position);
        let y0 = min - base / 2.0;
        Some(Rect::new(
            self.rest.x0,
            y0,
            self.rest.x1,
            y0 + (max - min) + base,
        ))
    }

    /// Corner radius of the pill, held at the resting size.
    fn radius(&self) -> f64 {
        self.rest.width().min(self.rest.height()) / 2.0
    }
}

// ---------------------------------------------------------------------------
// Text runs (label / badge / section header)
// ---------------------------------------------------------------------------

type TypeToken = (f32, f32, f32, FontWeight);

/// The ink every run is shaped with; paint re-brushes it to the resolved role
/// color — the [`crate::navigation_rail`]/[`crate::navbar`] run convention.
const SHAPING_INK: Color = Color::BLACK;

/// Resolve a live themed [`TextStyle`] if one was threaded in, else build one
/// from `token`'s unthemed fallback — [`crate::navigation_rail`]'s own
/// `type_style` helper, duplicated per this crate's per-module convention for
/// this small utility.
fn type_style(themed: Option<&TextStyle>, token: TypeToken) -> TextStyle {
    let mut style = match themed {
        Some(style) => style.clone(),
        None => {
            let (size, line_height, letter_spacing, weight) = token;
            let mut style = TextStyle::new(size, SHAPING_INK);
            style.line_height = LineHeight::Absolute(line_height);
            style.letter_spacing = letter_spacing;
            style.weight = weight;
            style
        }
    };
    style.color = SHAPING_INK;
    style
}

/// A lazily shaped, paint-time-rebrushed text run — a label, a badge, or a
/// section header. The same idiom [`crate::navigation_rail`]'s own `TextRun`
/// carries, duplicated per this crate's per-module convention.
struct Run {
    content: String,
    layout: Option<TextLayout>,
    shaped_for: Option<(TextStyle, Option<f64>)>,
}

impl Run {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_for: None,
        }
    }

    /// Replace the run's text, invalidating any cached shaping. Returns
    /// whether anything changed.
    fn set_content(&mut self, content: &str) -> bool {
        if self.content == content {
            return false;
        }
        self.content = content.to_string();
        self.layout = None;
        self.shaped_for = None;
        true
    }

    /// Shape (or reuse) the run, fitted to `max_width` as a single ellipsized
    /// line when one is given.
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f64>) -> Size {
        let key = (style.clone(), max_width);
        if let Some(layout) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return layout.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = match max_width {
            Some(width) => text_ctx.layout_bounded(
                &self.content,
                style,
                Some(width as f32),
                Some(1),
                TextOverflow::Ellipsis,
            ),
            None => text_ctx.layout(&self.content, style, None),
        };
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(key);
        size
    }

    /// Paint at `origin` in `color`, overriding [`SHAPING_INK`]. A never-shaped
    /// run paints nothing.
    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

/// A destination row's foreground ink: `on_secondary_container` selected,
/// `on_surface_variant` otherwise (`destinationForegroundColor`). Unlike
/// [`mod@crate::navbar`]/[`mod@crate::navigation_rail`], this module resolves
/// the exact upstream role rather than the closest [`frust::authoring::ThemeTextColor`]
/// one: every label/badge here is a [`Run`], painted with an explicitly
/// resolved [`Color`] rather than deferred through a themed text role, so the
/// [`ThemeTextColor`](frust::authoring::ThemeTextColor) gap that forces the
/// sibling nav widgets onto `on_surface` never applies here.
fn foreground(theme: Option<&Theme>, selected: bool) -> Color {
    match (theme, selected) {
        (Some(theme), true) => theme.scheme().on_secondary_container,
        (Some(theme), false) => theme.scheme().on_surface_variant,
        (None, true) => ON_SECONDARY_CONTAINER,
        (None, false) => ON_SURFACE_VARIANT,
    }
}

/// Whether a widget-local `pos` lies within a `size`-sized box anchored at the
/// origin — mirrors [`crate::navbar`]'s helper of the same shape.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

// ---------------------------------------------------------------------------
// Public props
// ---------------------------------------------------------------------------

/// A destination shown in a navigation drawer — `M3ENavigationDestination`.
pub struct DrawerDestination<State: 'static> {
    icon: AnyView<State>,
    selected_icon: Option<AnyView<State>>,
    label: String,
    badge_label: Option<String>,
    show_badge: bool,
    semantic_label: Option<String>,
}

/// Create a destination showing `icon` beside `label`.
///
/// Tint is the supplied icon view's own responsibility, the same stance
/// [`crate::navbar`]'s and [`crate::navigation_rail`]'s icon slots take.
pub fn drawer_destination<State: 'static>(
    icon: impl View<State>,
    label: impl Into<String>,
) -> DrawerDestination<State> {
    DrawerDestination {
        icon: AnyView::new(icon),
        selected_icon: None,
        label: label.into(),
        badge_label: None,
        show_badge: false,
        semantic_label: None,
    }
}

impl<State: 'static> DrawerDestination<State> {
    /// A distinct icon to show while this destination is selected (falls back
    /// to the base icon).
    pub fn selected_icon(mut self, icon: impl View<State>) -> Self {
        self.selected_icon = Some(AnyView::new(icon));
        self
    }

    /// Trailing badge text, painted at the row's end in the same `labelLarge`
    /// role as the destination's own label (`badgeLabel`). Wins over
    /// [`Self::show_badge`] if both are set.
    pub fn badge_label(mut self, label: impl Into<String>) -> Self {
        self.badge_label = Some(label.into());
        self
    }

    /// A small dot badge at the row's end, when no [`Self::badge_label`] is
    /// set (`showBadge`) — see the [module docs](self)' Badges section for
    /// why this port paints it, unlike upstream's own dead field.
    pub fn show_badge(mut self, show: bool) -> Self {
        self.show_badge = show;
        self
    }

    /// The accessible name, when the visible label isn't the right one.
    pub fn semantic_label(mut self, label: impl Into<String>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// The icon view showing right now.
    fn active_icon(&self, selected: bool) -> &AnyView<State> {
        if selected {
            self.selected_icon.as_ref().unwrap_or(&self.icon)
        } else {
            &self.icon
        }
    }

    fn badge_text(&self) -> Option<&str> {
        self.badge_label.as_deref()
    }
}

/// A labeled group of destinations — the reference's
/// `M3ENavigationRailSection` shape, generalized onto the drawer's own
/// single-`headline` upstream. See the [module docs](self)' Sections section.
pub struct DrawerSection<State: 'static> {
    header: Option<String>,
    destinations: Vec<DrawerDestination<State>>,
}

/// Create an unlabeled section over `destinations` (chain
/// [`DrawerSection::header`] to label it).
pub fn drawer_section<State: 'static>(
    destinations: Vec<DrawerDestination<State>>,
) -> DrawerSection<State> {
    DrawerSection {
        header: None,
        destinations,
    }
}

impl<State: 'static> DrawerSection<State> {
    /// Label this section (`titleSmall`, `onSurfaceVariant`) — upstream's
    /// `headline`, repeatable per section.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }
}

// ---------------------------------------------------------------------------
// The row (one destination's retained content)
// ---------------------------------------------------------------------------

/// A view-held, typed selection callback (erased per-destination on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// Erase `on_select` into a per-destination callback that always reports
/// `index` — the [`crate::navbar`]/[`crate::navigation_rail`] closed-over-index
/// adapter.
fn row_on_select<State: 'static>(on_select: &OnSelect<State>, index: usize) -> ErasedCallback {
    let callback = on_select.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state, index);
    })
}

/// One destination's retained content — a plain [`Widget`] (not
/// `View`/[`AnyView`]-erased; there is only ever one concrete row type, the
/// [`crate::navbar`]/[`crate::navigation_rail`] rationale). Paints only its own
/// icon/label/badge — the shared pill is [`DrawerContentWidget`]'s, painted
/// under every row.
struct DrawerRowWidget {
    icon: ChildPod,
    label: Run,
    label_text: String,
    label_origin: Point,
    badge: Run,
    /// Whether the trailing slot paints a dot instead of `badge`'s text —
    /// true only when `show_badge` was set and no `badge_label` was.
    badge_dot: bool,
    badge_size: Size,
    badge_origin: Point,
    selected: bool,
    semantic_label: Option<String>,
    on_select: ErasedCallback,
    /// Armed by a `Down`, cleared on `Up`/`Cancel` — fire-on-up-inside, no
    /// pressed visual (the pill is the only selection feedback, matching the
    /// sibling nav widgets' no-ink-splash stance).
    captured: bool,
}

/// Build one destination's retained [`ChildPod`].
fn build_row<State: 'static>(
    destination: &DrawerDestination<State>,
    selected: bool,
    on_select: &OnSelect<State>,
    index: usize,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let widget = DrawerRowWidget {
        icon: build_child(destination.active_icon(selected), ctx),
        label: Run::new(destination.label.clone()),
        label_text: destination.label.clone(),
        label_origin: Point::ZERO,
        badge: Run::new(destination.badge_text().unwrap_or("").to_string()),
        badge_dot: destination.badge_label.is_none() && destination.show_badge,
        badge_size: Size::ZERO,
        badge_origin: Point::ZERO,
        selected,
        semantic_label: destination.semantic_label.clone(),
        on_select: row_on_select::<State>(on_select, index),
        captured: false,
    };
    ChildPod::new(Box::new(widget))
}

/// Synthesize a [`PointerPhase::Cancel`] into a still-armed row pod — mirrors
/// [`crate::navbar`]'s `cancel_item` (a removed row's [`DrawerRowWidget`]
/// isn't `AnyView`-wrapped, so it can't go through [`teardown_child`]'s
/// downcast).
fn cancel_row(pod: &mut ChildPod) {
    let mut dummy_state = ();
    let mut ctx = EventCtx::new(&mut dummy_state, pod.origin(), pod.size());
    let cancel = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::ZERO,
        button: PointerButton::Primary,
    });
    pod.event_child(&mut ctx, &cancel);
}

impl Widget for DrawerRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            DESTINATION_HEIGHT
        };

        let theme = Theme::from_layout_ctx(ctx);
        let style = type_style(theme.map(|t| &t.type_scale.label_large), LABEL_LARGE);

        // The badge's own box first — the label is fitted around whatever it
        // leaves behind, the same order [`crate::navigation_rail`] resolves in.
        self.badge_size = if self.badge_dot {
            Size::new(BADGE_DOT, BADGE_DOT)
        } else if !self.badge.content.is_empty() {
            self.badge.shape(ctx, &style, None)
        } else {
            Size::ZERO
        };

        let inner_w = (width - 2.0 * DESTINATION_INNER_PAD_H).max(0.0);
        let badge_slot = if self.badge_size.width > 0.0 {
            self.badge_size.width + ICON_LABEL_GAP
        } else {
            0.0
        };
        let label_max_w = (inner_w - ICON_SIZE - ICON_LABEL_GAP - badge_slot).max(0.0);
        let label_size = self.label.shape(ctx, &style, Some(label_max_w));

        let icon_size = self
            .icon
            .layout_child(ctx, &BoxConstraints::loose(Size::new(ICON_SIZE, ICON_SIZE)));
        self.icon.set_origin(Point::new(
            DESTINATION_INNER_PAD_H,
            ((height - icon_size.height) / 2.0).max(0.0),
        ));

        self.label_origin = Point::new(
            DESTINATION_INNER_PAD_H + ICON_SIZE + ICON_LABEL_GAP,
            ((height - label_size.height) / 2.0).max(0.0),
        );
        self.badge_origin = Point::new(
            (width - DESTINATION_INNER_PAD_H - self.badge_size.width).max(0.0),
            ((height - self.badge_size.height) / 2.0).max(0.0),
        );

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let ink = foreground(theme, self.selected);
        let origin = ctx.origin();

        self.icon.paint_child(ctx, scene);
        self.label
            .paint(origin + self.label_origin.to_vec2(), ink, scene);
        if self.badge_dot {
            scene.fill_rounded_rect(
                origin + self.badge_origin.to_vec2(),
                self.badge_size,
                self.badge_size.height / 2.0,
                ink,
            );
        } else if !self.badge.content.is_empty() {
            self.badge
                .paint(origin + self.badge_origin.to_vec2(), ink, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
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
                if inside(p.position, ctx.size()) {
                    (self.on_select)(ctx);
                }
                self.captured = false;
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Tab, |node| {
            let label = self.semantic_label.as_deref().unwrap_or(&self.label_text);
            node.set_label(label);
            node.set_selected(self.selected);
        });
    }

    visit_children!(icon);
}

// ---------------------------------------------------------------------------
// The content widget (sections, dividers, the shared pill)
// ---------------------------------------------------------------------------

/// One section's resolved position in the drawer's flat destination list,
/// plus its header run — the [`crate::navigation_rail`]'s own `SectionLayout`
/// shape.
struct SectionLayout {
    /// Flat index of this section's first destination.
    first: usize,
    /// How many destinations this section holds.
    count: usize,
    header: Option<Run>,
    /// Where the header paints, in drawer-local space (set in `layout`).
    header_origin: Point,
    /// Whether a divider paints above this section — every section but the
    /// first (see the [module docs](self)' Sections section).
    divider: bool,
    /// Where the divider paints, in drawer-local space (set in `layout`).
    divider_origin: Point,
}

/// The retained widget for [`DrawerContentView`]. See the [module docs](self).
pub struct DrawerContentWidget {
    /// Every destination, flattened across sections — each pod wraps a
    /// [`DrawerRowWidget`].
    rows: Vec<ChildPod>,
    sections: Vec<SectionLayout>,
    /// The app-confirmed selection (see the [module docs](self)' controlled
    /// contract).
    selected: usize,
    indicator: DrawerIndicator,
    /// Raised by `rebuild` when the selection moved, consumed by the next
    /// `layout` — the pass that knows where the destination *is*.
    selection_moved: bool,
}

impl DrawerContentWidget {
    /// The selected row's resting box in drawer-local px, or `None` when the
    /// drawer holds no destinations.
    fn selected_row_rect(&mut self) -> Option<Rect> {
        let pod = self.rows.get_mut(self.selected)?;
        Some(Rect::from_origin_size(pod.origin(), pod.size()))
    }

    /// An estimate of the drawer's own content height, used only as the
    /// unbounded-height layout fallback (a mount with no bounded viewport at
    /// all) — the real layout pass always positions every row/header/divider
    /// precisely against the resolved width.
    fn natural_height(&self) -> f64 {
        let mut h = 0.0;
        for section in &self.sections {
            if section.divider {
                h += DIVIDER_THICKNESS;
            }
            if section.header.is_some() {
                h += HEADER_ROW_HEIGHT;
            }
            h += section.count as f64 * ROW_OUTER_HEIGHT;
        }
        h
    }
}

/// A declarative navigation-drawer content list: sections of destinations
/// sharing the liquid selection indicator — the **standard, non-modal,
/// layout-participating** entry point. See the [module docs](self).
pub struct DrawerContentView<State: 'static> {
    sections: Vec<DrawerSection<State>>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Build the drawer's bare content — upstream's own `M3ENavigationDrawer`
/// shape, with no scrim/dismiss of its own (see the [module docs](self)).
/// `selected` is the current (app-confirmed) **flat** destination index,
/// counted across every section in order; fires `on_select(state, index)` on
/// a release inside a destination.
pub fn navigation_drawer_content<State: 'static, F: Fn(&mut State, usize) + 'static>(
    sections: Vec<DrawerSection<State>>,
    selected: usize,
    on_select: F,
) -> DrawerContentView<State> {
    DrawerContentView {
        sections,
        selected,
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> DrawerContentView<State> {
    /// Every destination, flattened across sections in order — the index
    /// space `selected`/`on_select` speak.
    fn destinations(&self) -> Vec<&DrawerDestination<State>> {
        self.sections
            .iter()
            .flat_map(|section| section.destinations.iter())
            .collect()
    }
}

impl<State: 'static> View<State> for DrawerContentView<State> {
    type Element = DrawerContentWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DrawerContentWidget {
        let mut rows = Vec::new();
        let mut sections = Vec::new();
        for section in &self.sections {
            sections.push(SectionLayout {
                first: rows.len(),
                count: section.destinations.len(),
                header: section.header.as_ref().map(Run::new),
                header_origin: Point::ZERO,
                divider: !sections.is_empty(),
                divider_origin: Point::ZERO,
            });
            for destination in &section.destinations {
                let index = rows.len();
                rows.push(build_row(
                    destination,
                    index == self.selected,
                    &self.on_select,
                    index,
                    ctx,
                ));
            }
        }
        DrawerContentWidget {
            rows,
            sections,
            selected: self.selected,
            indicator: DrawerIndicator::new(),
            selection_moved: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DrawerContentWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let prev_dests = prev.destinations();
        let next_dests = self.destinations();
        let common = prev_dests.len().min(next_dests.len());

        for index in 0..common {
            let was_selected = index == prev.selected;
            let now_selected = index == self.selected;
            let pod = &mut element.rows[index];
            let widget = pod
                .widget_mut()
                .downcast_mut::<DrawerRowWidget>()
                .expect("a drawer row pod holds a DrawerRowWidget");

            flags |= rebuild_child(
                prev_dests[index].active_icon(was_selected),
                next_dests[index].active_icon(now_selected),
                &mut widget.icon,
                ctx,
            );
            if widget.label.set_content(&next_dests[index].label) {
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            widget.label_text = next_dests[index].label.clone();
            let next_badge = next_dests[index].badge_text().unwrap_or("");
            if widget.badge.set_content(next_badge) {
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            let next_dot = next_dests[index].badge_label.is_none() && next_dests[index].show_badge;
            if widget.badge_dot != next_dot {
                widget.badge_dot = next_dot;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            widget.semantic_label = next_dests[index].semantic_label.clone();
            if widget.selected != now_selected {
                widget.selected = now_selected;
                flags |= ChangeFlags::PAINT;
            }
            // Closures aren't comparable — always reinstall the adapter.
            widget.on_select = row_on_select::<State>(&self.on_select, index);
        }

        if next_dests.len() > prev_dests.len() {
            for (offset, destination) in next_dests[common..].iter().enumerate() {
                let index = common + offset;
                element.rows.push(build_row(
                    destination,
                    index == self.selected,
                    &self.on_select,
                    index,
                    ctx,
                ));
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if next_dests.len() < prev_dests.len() {
            for (offset, destination) in prev_dests[common..].iter().enumerate() {
                let index = common + offset;
                let pod = &mut element.rows[index];
                if pod.is_active() {
                    cancel_row(pod);
                    pod.set_active(false);
                }
                let widget = pod
                    .widget_mut()
                    .downcast_mut::<DrawerRowWidget>()
                    .expect("a drawer row pod holds a DrawerRowWidget");
                teardown_child(
                    destination.active_icon(index == prev.selected),
                    &mut widget.icon,
                    ctx,
                );
            }
            element.rows.truncate(common);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let sections_changed = prev.sections.len() != self.sections.len()
            || prev
                .sections
                .iter()
                .zip(self.sections.iter())
                .any(|(a, b)| a.destinations.len() != b.destinations.len() || a.header != b.header);
        if sections_changed {
            let mut first = 0;
            element.sections = self
                .sections
                .iter()
                .enumerate()
                .map(|(i, section)| {
                    let layout = SectionLayout {
                        first,
                        count: section.destinations.len(),
                        header: section.header.as_ref().map(Run::new),
                        header_origin: Point::ZERO,
                        divider: i > 0,
                        divider_origin: Point::ZERO,
                    };
                    first += section.destinations.len();
                    layout
                })
                .collect();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if prev.selected != self.selected {
            element.selected = self.selected;
            // The indicator's travel starts in `layout` (the pass that knows
            // where the destination *is*), so a moved selection must ask for
            // one — a bare repaint would leave the pill parked.
            element.selection_moved = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut DrawerContentWidget, ctx: &mut BuildCtx<'_>) {
        for (index, destination) in self.destinations().iter().enumerate() {
            let Some(pod) = element.rows.get_mut(index) else {
                continue;
            };
            let widget = pod
                .widget_mut()
                .downcast_mut::<DrawerRowWidget>()
                .expect("a drawer row pod holds a DrawerRowWidget");
            teardown_child(
                destination.active_icon(index == self.selected),
                &mut widget.icon,
                ctx,
            );
        }
    }
}

impl Widget for DrawerContentWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let reduce_motion = Theme::from_layout_ctx(ctx).is_some_and(|t| t.motion.reduce_motion);
        let travel = std::mem::take(&mut self.selection_moved) && !reduce_motion;

        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            self.natural_height()
        };
        // Self-drives to DRAWER_WIDTH regardless of mount (a standalone
        // layout-participating mount gets upstream's own `Container(width:
        // 360)` behavior for free); a modal host already caps its incoming
        // `bc.max().width` at DRAWER_WIDTH, so this clamps to a no-op there.
        let size = bc.constrain(Size::new(DRAWER_WIDTH, height));
        let width = size.width;

        let mut sections = std::mem::take(&mut self.sections);
        let mut y = 0.0;
        for section in &mut sections {
            if section.divider {
                section.divider_origin = Point::new(0.0, y);
                y += DIVIDER_THICKNESS;
            }
            if let Some(header) = section.header.as_mut() {
                let theme = Theme::from_layout_ctx(ctx);
                let style = type_style(theme.map(|t| &t.type_scale.title_small), TITLE_SMALL);
                let max_w = (width - 2.0 * HEADLINE_PAD_H).max(0.0);
                let header_size = header.shape(ctx, &style, Some(max_w));
                section.header_origin = Point::new(HEADLINE_PAD_H, y + HEADLINE_PAD_V);
                y += header_size.height + 2.0 * HEADLINE_PAD_V;
            }
            let row_w = (width - 2.0 * DESTINATION_PAD_H).max(0.0);
            for index in section.first..section.first + section.count {
                let Some(pod) = self.rows.get_mut(index) else {
                    continue;
                };
                pod.layout_child(
                    ctx,
                    &BoxConstraints::tight(Size::new(row_w, DESTINATION_HEIGHT)),
                );
                pod.set_origin(Point::new(DESTINATION_PAD_H, y + DESTINATION_PAD_V));
                y += ROW_OUTER_HEIGHT;
            }
        }
        self.sections = sections;

        if let Some(rect) = self.selected_row_rect() {
            self.indicator.sync(rect, travel);
        }

        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let indicator_fill = theme.map_or(SECONDARY_CONTAINER, |t| t.scheme().secondary_container);
        let header_ink = theme.map_or(ON_SURFACE_VARIANT, |t| t.scheme().on_surface_variant);
        let divider_ink = theme.map_or(OUTLINE_VARIANT, |t| t.scheme().outline_variant);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();
        let width = ctx.size().width;

        // The pill is paint-only geometry: no row's layout depends on it, so
        // a plain frame request is enough (the [`crate::navbar`] precedent).
        if reduce_motion {
            self.indicator.settle();
        } else if self.indicator.advance(ctx.frame_time()) {
            ctx.request_frame();
        }
        if let Some(pill) = self.indicator.pill_rect() {
            scene.fill_rounded_rect(
                origin + Vec2::new(pill.x0, pill.y0),
                pill.size(),
                self.indicator.radius(),
                indicator_fill,
            );
        }

        for section in &self.sections {
            if section.divider {
                scene.fill_rect(
                    origin + Vec2::new(0.0, section.divider_origin.y),
                    Size::new(width, DIVIDER_THICKNESS),
                    divider_ink,
                );
            }
            if let Some(header) = &section.header {
                header.paint(origin + section.header_origin.to_vec2(), header_ink, scene);
            }
        }
        for pod in &mut self.rows {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.rows, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for pod in &self.rows {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(rows);
}

// ---------------------------------------------------------------------------
// The modal entry point
// ---------------------------------------------------------------------------

/// The M3 navigation-drawer chrome: left-edge-pinned (the leading edge in
/// LTR), fixed [`DRAWER_WIDTH`] wide, full height, right corners only (the
/// host's own `OverlaySide::Left` → `OverlayCorners::End` mapping — no
/// override needed), sliding in with the host's own [`frust::authoring`]
/// default entrance. See the [module docs](self).
fn drawer_config() -> OverlayModalConfig {
    OverlayModalConfig::edge(OverlaySide::Left)
        .extent(OverlayExtent::Content, OverlayLimit::Px(DRAWER_WIDTH))
}

/// A declarative modal M3 navigation drawer: [`DrawerContentView`] wrapped
/// around the merged [`crate::overlay::modal`] host. See the [module
/// docs](self).
pub struct NavigationDrawerView<State: 'static>(OverlayModalView<State>);

/// Build a modal navigation drawer over `sections`. See [`show_navigation_drawer`]
/// for the navigator-pushed convenience most callers want, and
/// [`navigation_drawer_content`] for the bare, non-modal content this wraps.
pub fn navigation_drawer<State: 'static, F: Fn(&mut State, usize) + 'static>(
    sections: Vec<DrawerSection<State>>,
    selected: usize,
    on_select: F,
) -> NavigationDrawerView<State> {
    let content = navigation_drawer_content(sections, selected, on_select);
    NavigationDrawerView(overlay_modal(content, drawer_config()).label("Navigation drawer"))
}

/// PascalCase alias for [`navigation_drawer`], matching the catalog's
/// container view-fn vocabulary.
#[allow(non_snake_case)]
pub fn NavigationDrawer<State: 'static, F: Fn(&mut State, usize) + 'static>(
    sections: Vec<DrawerSection<State>>,
    selected: usize,
    on_select: F,
) -> NavigationDrawerView<State> {
    navigation_drawer(sections, selected, on_select)
}

impl<State: 'static> NavigationDrawerView<State> {
    /// Whether the user can dismiss this drawer at all — the scrim tap,
    /// `Escape`, and an Android back press (default `true`; see
    /// [`mod@crate::overlay::modal`]'s module docs). `false` disables all of
    /// them; only an explicit control the app wires through its own content
    /// (or a selection's own `on_select`) still dismisses it.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.0 = self.0.dismissable(dismissable);
        self
    }

    /// Set the **unstaged** dismiss callback — delivered with `&mut State`
    /// during the event pass. [`show_navigation_drawer`] wires the staged
    /// (navigator-pop, slide-out) path instead; set this only for a
    /// `Stack`-mounted drawer with no navigator underneath it.
    pub fn on_dismiss<F: Fn(&mut State) + 'static>(mut self, on_dismiss: F) -> Self {
        self.0 = self.0.on_dismiss(on_dismiss);
        self
    }
}

impl<State: 'static> View<State> for NavigationDrawerView<State> {
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

impl<State: 'static> OverlayModalContent<State> for NavigationDrawerView<State> {
    fn on_modal_dismiss(mut self, on_dismiss: Rc<dyn Fn(&mut State)>) -> Self {
        self.0 = self.0.on_modal_dismiss(on_dismiss);
        self
    }

    fn modal_dismissable(&self) -> bool {
        self.0.modal_dismissable()
    }
}

/// Push `build`'s navigation drawer as a transparent navigator page (the page
/// below stays visible under the scrim), sliding in from the leading edge,
/// and register `on_result` for the value it pops with — the
/// [`crate::overlay::modal`] host's shared [`show_overlay_modal`] push, the
/// same shape as [`crate::side_sheet::show_side_sheet`]. The scrim tap,
/// `Escape`, and an Android back press are all wired to `controller.pop()`
/// for you, staged behind the slide-out exit ramp — selecting a destination
/// is **not** one of them (see the [module docs](self)' Selecting does not
/// dismiss section); an app that wants that UX pops from inside its own
/// `on_select`, but that pop is **unstaged** (no reverse-ramp exit) —
/// `docs/LIMITATIONS.md`'s `material-modal-staged-dismiss-private-to-host`.
///
/// ```ignore
/// show_navigation_drawer(
///     &state.nav,
///     || navigation_drawer(sections.clone(), state.drawer_index, |s: &mut State, i| {
///         s.drawer_index = i;
///         // Unstaged: dismiss-on-select is an app decision, not the
///         // default, and this pop skips the staged reverse-ramp exit
///         // every host-chrome dismiss gesture on this drawer takes.
///         s.nav.pop();
///     }),
///     |state: &mut State, _result: PopResult| {},
/// );
/// ```
pub fn show_navigation_drawer<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> NavigationDrawerView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    show_overlay_modal(controller, build, on_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BuildCtx, CornerRadii, EventCtx, PointerButton, PointerEvent, PointerPhase, Role, Widget,
        any,
    };
    use frust::{FrameTime, NavigatorView, icon};
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use kurbo::{Point, Rect, Size};
    use peniko::Color;
    use std::any::Any;

    const WINDOW: Size = Size::new(400.0, 600.0);

    fn sample_destination<State: 'static>(label: &str) -> DrawerDestination<State> {
        drawer_destination(any(icon(crate::icons::HOME)), label)
    }

    fn one_section<State: 'static>(labels: &[&str]) -> Vec<DrawerSection<State>> {
        vec![drawer_section(
            labels.iter().map(|l| sample_destination(l)).collect(),
        )]
    }

    fn build<S: 'static>(view: &DrawerContentView<S>) -> DrawerContentWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DrawerContentWidget, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, bc)
    }

    fn ft(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    // ---- Modal config: geometry, corners. ----------------------------------

    #[test]
    fn drawer_config_pins_a_left_edge_fixed_width_panel_with_end_corners_only() {
        let config = drawer_config();
        assert_eq!(
            config.geometry,
            crate::overlay::OverlayGeometry::Edge {
                side: OverlaySide::Left,
                extent: OverlayExtent::Content,
                limit: OverlayLimit::Px(DRAWER_WIDTH),
            }
        );
        assert_eq!(
            config.corners,
            crate::overlay::OverlayCorners::End,
            "a left-pinned panel rounds its right (screen-facing) corners only"
        );
        assert_eq!(
            config.entrance,
            crate::overlay::OverlayEntrance::Slide,
            "a drawer slides in from its own edge"
        );
        assert!(!config.close_button, "upstream paints no close affordance");
        assert!(
            !config.drag,
            "no edge-swipe-to-open exists upstream to port"
        );
    }

    #[test]
    fn unthemed_modal_paint_rounds_only_the_right_corners() {
        let view: NavigationDrawerView<()> = {
            let content = navigation_drawer_content(one_section(&["Home"]), 0, |_: &mut (), _| {});
            NavigationDrawerView(
                overlay_modal(
                    content,
                    drawer_config().entrance(crate::overlay::OverlayEntrance::None),
                )
                .label("Navigation drawer"),
            )
        };
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));

        #[derive(Default)]
        struct Recorder {
            panels: Vec<(Point, Size, CornerRadii, Color)>,
        }
        impl PaintScene for Recorder {
            fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
            fn draw_text(&mut self, _o: Point, _t: &str) {}
            fn fill_rounded_rect_radii(
                &mut self,
                origin: Point,
                size: Size,
                radii: CornerRadii,
                color: Color,
            ) {
                self.panels.push((origin, size, radii, color));
            }
            fn push_clip_rounded_radii(&mut self, _o: Point, _s: Size, _r: CornerRadii) {}
            fn push_clip(&mut self, _o: Point, _s: Size) {}
            fn pop_clip(&mut self) {}
        }
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, WINDOW);
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.panels.len(), 1);
        let (origin, size, radii, _) = rec.panels[0];
        assert_eq!(origin.x, 0.0, "pinned to the leading (left) edge");
        assert_eq!(size.width, DRAWER_WIDTH);
        assert_eq!(radii.top_left, 0.0, "left corners are square (screen edge)");
        assert_eq!(
            radii.bottom_left, 0.0,
            "left corners are square (screen edge)"
        );
        assert!(radii.top_right > 0.0, "right corners round");
        assert!(radii.bottom_right > 0.0, "right corners round");
    }

    // ---- Standalone content: self-sizes to DRAWER_WIDTH either way. -------

    #[test]
    fn the_standalone_content_self_sizes_to_drawer_width_under_a_loose_or_wide_bounded_offer() {
        let view: DrawerContentView<()> =
            navigation_drawer_content(one_section(&["Home", "Search"]), 0, |_: &mut (), _| {});
        let mut w = build(&view);
        let loose = layout(
            &mut w,
            &BoxConstraints::loose(Size::new(f64::INFINITY, 600.0)),
        );
        assert_eq!(loose.width, DRAWER_WIDTH);

        let mut w2 = build(&view);
        let wide = layout(&mut w2, &BoxConstraints::loose(Size::new(900.0, 600.0)));
        assert_eq!(
            wide.width, DRAWER_WIDTH,
            "a fixed-width panel does not stretch to fill a wider offer"
        );
    }

    #[test]
    fn the_standalone_content_clamps_down_when_the_offer_is_narrower_than_drawer_width() {
        let view: DrawerContentView<()> =
            navigation_drawer_content(one_section(&["Home"]), 0, |_: &mut (), _| {});
        let mut w = build(&view);
        let size = layout(&mut w, &BoxConstraints::tight(Size::new(250.0, 600.0)));
        assert_eq!(size.width, 250.0);
    }

    // ---- Sections/dividers/headers. ----------------------------------------

    #[test]
    fn a_single_section_paints_no_divider_and_positions_rows_top_to_bottom() {
        let view: DrawerContentView<()> = navigation_drawer_content(
            one_section(&["Home", "Search", "Agenda"]),
            0,
            |_: &mut (), _| {},
        );
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(WINDOW));

        assert!(!w.sections[0].divider);
        assert_eq!(w.rows[0].origin().y, DESTINATION_PAD_V);
        assert_eq!(w.rows[1].origin().y, DESTINATION_PAD_V + ROW_OUTER_HEIGHT);
        assert_eq!(
            w.rows[2].origin().y,
            DESTINATION_PAD_V + 2.0 * ROW_OUTER_HEIGHT
        );
    }

    #[test]
    fn a_second_section_paints_a_divider_and_an_optional_header_above_its_own_rows() {
        let sections: Vec<DrawerSection<()>> = vec![
            drawer_section(vec![sample_destination("Home")]),
            drawer_section(vec![sample_destination("Settings")]).header("More"),
        ];
        let view: DrawerContentView<()> =
            navigation_drawer_content(sections, 0, |_: &mut (), _| {});
        let mut w = build(&view);
        layout(&mut w, &BoxConstraints::tight(WINDOW));

        assert!(
            !w.sections[0].divider,
            "the first section never gets a divider"
        );
        assert!(w.sections[1].divider, "every later section does");
        assert!(w.sections[1].header.is_some());
        // Second section starts after: one row (60) + a 1px divider + the
        // header's own vertical padding band.
        let expected_second_row_y =
            ROW_OUTER_HEIGHT + DIVIDER_THICKNESS + HEADER_ROW_HEIGHT + DESTINATION_PAD_V;
        assert_eq!(w.rows[1].origin().y, expected_second_row_y);
    }

    // ---- Selection: press fires on_select, indicator travels. -------------

    /// Dispatch one event through a fresh [`EventCtx`] each call, so `state`
    /// is freely readable between dispatches (the [`mod@crate::side_sheet`]
    /// test precedent).
    fn dispatch<S: 'static>(
        w: &mut DrawerContentWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    fn press_at(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn pressing_a_row_fires_on_select_with_its_flat_index_on_up_inside_only() {
        #[derive(Default)]
        struct S {
            selected: Option<usize>,
        }
        let view: DrawerContentView<S> = navigation_drawer_content(
            vec![drawer_section(vec![
                sample_destination("Home"),
                sample_destination("Search"),
            ])],
            0,
            |s: &mut S, i| s.selected = Some(i),
        );
        let mut counter = 0u64;
        let mut w = View::<S>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, &BoxConstraints::tight(WINDOW));

        let rect = Rect::from_origin_size(w.rows[1].origin(), w.rows[1].size());
        let center = rect.center();
        let mut state = S::default();
        dispatch(&mut w, &mut state, &press_at(PointerPhase::Down, center));
        assert_eq!(state.selected, None, "no fire on Down");
        dispatch(&mut w, &mut state, &press_at(PointerPhase::Up, center));
        assert_eq!(
            state.selected,
            Some(1),
            "fires with the pressed row's index"
        );
    }

    #[test]
    fn selecting_a_new_row_travels_the_indicator_to_its_settled_rect() {
        let view0: DrawerContentView<()> =
            navigation_drawer_content(one_section(&["Home", "Search"]), 0, |_: &mut (), _| {});
        let view1: DrawerContentView<()> =
            navigation_drawer_content(one_section(&["Home", "Search"]), 1, |_: &mut (), _| {});

        let mut counter = 0u64;
        let mut w = View::<()>::build(&view0, &mut BuildCtx::new(&mut counter));
        layout(&mut w, &BoxConstraints::tight(WINDOW));
        // Settle the initial jump.
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, ft(0));
        let mut scene = frust_widgets::test_support::RecordingScene::default();
        w.paint(&mut pctx, &mut scene);

        View::<()>::rebuild(&view1, &view0, &mut w, &mut BuildCtx::new(&mut counter));
        layout(&mut w, &BoxConstraints::tight(WINDOW));
        let mut pctx = PaintCtx::for_test(Point::ZERO, WINDOW, ft(50));
        w.paint(&mut pctx, &mut scene);
        layout(&mut w, &BoxConstraints::tight(WINDOW));
        let mut pctx2 = PaintCtx::for_test(Point::ZERO, WINDOW, ft(2000));
        w.paint(&mut pctx2, &mut scene);

        let expected = Rect::from_origin_size(w.rows[1].origin(), w.rows[1].size());
        let pill = w.indicator.pill_rect().expect("indicator is ready");
        assert!((pill.y0 - expected.y0).abs() < 0.5);
        assert!((pill.y1 - expected.y1).abs() < 0.5);
    }

    // ---- Indicator sync gate: an interleaved no-op layout must not abort a
    // travel already in flight (a relayout pass that reports the *same*
    // resting geometry, `travel: false`, is not a geometry change). --------

    #[test]
    fn an_interleaved_non_travel_sync_at_the_same_geometry_does_not_abort_an_in_flight_travel() {
        let mut indicator = DrawerIndicator::new();
        let rest0 = Rect::from_origin_size(Point::new(0.0, 0.0), Size::new(300.0, 56.0));
        indicator.sync(rest0, false);
        assert!(!indicator.is_animating(), "first layout jumps, not travels");

        let rest1 = Rect::from_origin_size(Point::new(0.0, 56.0), Size::new(300.0, 56.0));
        indicator.sync(rest1, true);
        assert!(
            indicator.is_animating(),
            "a selection change starts an in-flight travel"
        );

        indicator.advance(ft(50));
        assert!(
            indicator.is_animating(),
            "still travelling after a partial advance"
        );

        // An unrelated relayout mid-travel reports the same geometry with
        // travel=false (selection.moved was already consumed by the prior
        // layout pass) — this must not snap the pill onto its target and
        // abort the stretch/settle animation.
        indicator.sync(rest1, false);
        assert!(
            indicator.is_animating(),
            "an interleaved no-op layout at the same geometry must not abort \
             an in-flight travel"
        );

        indicator.advance(ft(2000));
        assert!(!indicator.is_animating(), "travel completes normally");
        let pill = indicator.pill_rect().expect("indicator is ready");
        assert!((pill.center().y - rest1.center().y).abs() < 0.5);
    }

    #[test]
    fn a_non_travel_sync_at_a_genuinely_changed_geometry_still_jumps() {
        let mut indicator = DrawerIndicator::new();
        let rest0 = Rect::from_origin_size(Point::new(0.0, 0.0), Size::new(300.0, 56.0));
        indicator.sync(rest0, false);

        let rest1 = Rect::from_origin_size(Point::new(0.0, 56.0), Size::new(300.0, 56.0));
        indicator.sync(rest1, true);
        indicator.advance(ft(50));
        assert!(indicator.is_animating());

        // A real geometry change (e.g. a width/height resize moving the
        // resting box to a different center) reported with travel=false must
        // still jump immediately, not continue the stale travel.
        let rest2 = Rect::from_origin_size(Point::new(0.0, 200.0), Size::new(300.0, 56.0));
        indicator.sync(rest2, false);
        assert!(
            !indicator.is_animating(),
            "a genuine geometry change jumps rather than travelling"
        );
        let pill = indicator.pill_rect().expect("indicator is ready");
        assert!((pill.center().y - rest2.center().y).abs() < 0.5);
    }

    // ---- Badges: text wins over a dot; a dot paints when text is absent. --

    /// Mirrors [`build_row`]'s own `badge_dot` derivation — a `badge_label`
    /// always wins over `show_badge`.
    fn badge_dot_would_paint<State: 'static>(d: &DrawerDestination<State>) -> bool {
        d.badge_label.is_none() && d.show_badge
    }

    #[test]
    fn a_badge_label_paints_trailing_text_and_a_dot_badge_paints_when_no_label_is_set() {
        let with_text: DrawerDestination<()> =
            drawer_destination(any(icon(crate::icons::HOME)), "Search").badge_label("3");
        assert!(!badge_dot_would_paint(&with_text));

        let with_dot: DrawerDestination<()> =
            drawer_destination(any(icon(crate::icons::HOME)), "Search").show_badge(true);
        assert!(badge_dot_would_paint(&with_dot));

        let both: DrawerDestination<()> =
            drawer_destination(any(icon(crate::icons::HOME)), "Search")
                .badge_label("3")
                .show_badge(true);
        assert!(
            !badge_dot_would_paint(&both),
            "a badge_label wins over show_badge"
        );
    }

    // ---- Semantics: TabList container, one Tab per row. --------------------

    #[test]
    fn semantics_is_a_tab_list_with_the_selected_row_flagged() {
        fn logic(_s: &mut ()) -> DrawerContentView<()> {
            navigation_drawer_content(one_section(&["Home", "Search"]), 1, |_: &mut (), _| {})
        }
        let mut root: RenderRoot<(), DrawerContentView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();

        assert!(update.nodes.iter().any(|(_, n)| n.role() == Role::TabList));
        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 2);
        assert!(tabs.iter().any(|(_, n)| n.label() == Some("Home")));
        let selected = tabs
            .iter()
            .find(|(_, n)| n.label() == Some("Search"))
            .expect("the second destination carries its own label");
        assert_eq!(selected.1.is_selected(), Some(true));
    }

    // ---- Modal push + staged scrim-tap dismiss. -----------------------------

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<i32>>,
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
    fn bg_page(w: f64, h: f64) -> BgPage {
        BgPage {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn show_navigation_drawer_stages_a_scrim_tap_dismiss_through_the_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = {
            let ctrl = controller.clone();
            move |_: &mut NavState| {
                navigator(&ctrl, || {
                    any::<NavState, _>(bg_page(WINDOW.width, WINDOW.height))
                })
            }
        };
        let mut state = NavState::default();

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        show_navigation_drawer(
            &controller,
            || navigation_drawer(one_section(&["Home"]), 0, |_: &mut NavState, _| {}),
            |state: &mut NavState, result: PopResult| {
                state.results.push(result.take::<i32>());
            },
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        for t in [0u64, 100, 200, 300, 400, 550] {
            root.paint(
                &mut frust_widgets::test_support::RecordingScene::default(),
                ft(t),
            );
        }

        // Tap the trailing-edge scrim, outside the leading-pinned panel.
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(WINDOW.width - 5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                position: Point::new(WINDOW.width - 5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);

        let mut frames = 0u64;
        loop {
            root.rebuild(&mut app, &mut state);
            root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
            let outcome = root.paint(
                &mut frust_widgets::test_support::RecordingScene::default(),
                ft(600 + frames * 100),
            );
            frames += 1;
            assert!(
                frames < 30,
                "the staged exit settles within a bounded number of frames"
            );
            if !outcome.needs_frame {
                break;
            }
        }
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(WINDOW.width - 5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );

        assert_eq!(
            state.results,
            vec![None],
            "a scrim tap stages the exit and pops with an empty result"
        );
    }
}
