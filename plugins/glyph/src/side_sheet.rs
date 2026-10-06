// Composition ported from `material_3_expressive` v1.0.8's `M3ESideSheet`/
// `M3ESideSheetTheme` (MIT, © 2026 Paa Developments;
// `lib/components/side_sheets/m3e_side_sheets.dart`, per the attribution
// header `plugins/material/src/side_sheet.rs` records — retrieved 2026-08-20).
// Upstream: <https://github.com/paadevelopments/material_3_expressive>.
//
// Porting decisions: only the *composition* is carried over — a fixed-height
// header row (title + a close affordance), a body filling everything between,
// and an optional divider-plus-content footer pinned to the bottom edge. The
// modal chrome is not ported at all: scrim, edge-pinned slide, staged exit,
// back-dismiss and the programmatic-close handle are [`crate::sheet`]'s own,
// re-derived here on the x axis, and every metric, color, and motion value is
// Glyph's. The reference was read, never imported: a design-system plugin
// depends on no sibling design-system crate (`docs/PLUGINS_ARCHITECTURE.md`'s
// Design-System Plugins charter).

//! Glyph modal **side sheet**: a full-area scrim plus a trailing-edge-pinned,
//! full-height panel holding a sticky header, a body, and an optional sticky
//! footer — the x-axis sibling of [`crate::sheet`]'s bottom sheet, over the
//! same navigator transparent-push modal plumbing.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! A `GlyphSideSheetView` is pushed as a **transparent navigator page** via
//! [`NavigatorController::push_with_options`] with a
//! [`BackPolicy`](frust::BackPolicy) and a dismiss signal — the seam
//! [`show_glyph_sheet`](crate::sheet::show_glyph_sheet) uses. The page below
//! stays visible under the scrim, the navigator's modal contract makes it
//! modal, and [`show_glyph_side_sheet`] wraps the push, wiring dismissal to
//! `controller.pop()` **composed with** any
//! [`on_close`](GlyphSideSheetView::on_close) the caller set. Enforcing "at
//! most one modal at a time" stays the app's concern, like every other modal
//! in this catalog.
//!
//! # Enter/exit staging — the scrim fades, the panel slides (independently)
//!
//! Same hard requirement [`crate::sheet`] carries, on the other axis: the
//! scrim must fade while only the panel slides. So this widget drives its own
//! timeline from `PaintCtx::frame_time` and is pushed with
//! [`TransitionSpec::NONE`]:
//!
//! * the **scrim** is a plain full-area fill painted *before* any transform,
//!   its alpha driven straight off the timeline — it never moves, only fades;
//! * the **panel** is painted inside a single [`kurbo::Affine::translate`]
//!   pushed after the scrim, sliding in from fully off-screen (`x` = panel
//!   width to the right of rest) over the enter duration, reversing on exit.
//!
//! Enter runs over `durations.base` (220ms, spatial easing), exit over the
//! faster `durations.fast` (150ms, exit easing) — Glyph's own motion tokens,
//! which win over any millisecond figure the design mock names. Under
//! `reduce_motion` both collapse to the same short linear crossfade
//! [`crate::sheet`] collapses to. Every dismiss path (scrim tap, the header's
//! close affordance, `Escape`, an Android back press, and
//! [`GlyphSideSheetHandle::close`]) funnels through
//! [`begin_exit`](GlyphSideSheetWidget::begin_exit): no path pops the page
//! without playing the staged exit, and a dismissal mid-enter resumes from
//! wherever the entering panel had reached rather than snapping to rest first.
//!
//! **The barrier is only a barrier while it is on screen.** Its input swallow
//! is gated on the scrim's own progress, never on the phase, and the gate is
//! **one** test taken ahead of every event arm: with the scrim transparent —
//! the first frame of an entrance, and everything past a completed exit — a
//! press, an `Escape`, a wheel notch, a still-captured drag all fall through to
//! the page below instead of being swallowed by an invisible full-window
//! surface (`docs/REVIEW_FOCUS.md`'s named overlay defect). The **one**
//! deliberate exception is a broadcast (`InputEvent::Housekeeping`), which is
//! not user input, is never consumed, and must keep reaching the sheet's
//! content for as long as it is mounted so a deferred callback still flushes.
//! That matters because `Phase::Dismissed` is **terminal**: a
//! navigator-hosted sheet is unmounted by its own pop a frame later, but a
//! standalone-composed one (a bare [`glyph_side_sheet`], no
//! [`on_close`](GlyphSideSheetView::on_close)) just stays mounted there. For
//! the same reason frames are requested only while a ramp is running plus the
//! single frame the exit lands on, never for as long as the sheet is dismissed.
//!
//! # Geometry — trailing edge, full height, square corners
//!
//! The panel is `min(0.83 × viewport width, `[`GLYPH_SIDE_SHEET_MAX_WIDTH`]`)`
//! wide and full height, flush with the trailing (right) edge. Its corners are **square**:
//! the panel runs edge to edge, so there is no floating surface for a radius
//! to describe (where [`crate::dialog`]/[`crate::sheet`] both round the
//! corners that face into the screen). Its one edge treatment is a 1px
//! `border-bright` hairline down the leading edge, and it carries **no drop
//! shadow** — a full-height edge-pinned panel could only ever show one, and
//! the hairline already reads there.
//!
//! # Header / body / footer
//!
//! * **Header** — a fixed-height row (`HEADER_HEIGHT`), never scrolls: `title`
//!   (single-line, ellipsized, its family the live theme's `headlineSmall`
//!   role — Glyph's display face, Space Mono, under Glyph's own scale — via
//!   `Text`'s `.themed_family(..)`, so a theme swap repaints it; unthemed,
//!   `Text`'s own system family) plus the close affordance
//!   below, closed by a bottom `--border` hairline. The row's *content*
//!   (title, close chip, divider) sits below the window's top safe-area inset
//!   — the header band grows to `top_inset + HEADER_HEIGHT` — while the
//!   **panel itself stays full-bleed**, painting edge to edge under the
//!   status bar (the same "content shifts, chrome doesn't" split
//!   [`crate::appbar`]'s own top-inset consumption uses). A shell that never
//!   pushes insets (headless tests, a desktop preview without a
//!   window-inset source) sees `top_inset = 0.0` and the geometry below
//!   collapses to the pre-inset numbers exactly.
//! * **Body** — the caller's view, given the remaining height **tight** and
//!   inset horizontally by [`BODY_PAD_X`] on *both* edges (the approved
//!   mock's `.side-sheet-body` gutter) — clipped by this component, whose
//!   clip rect still spans the panel's full width. This inset is the
//!   component's own: a caller must not re-add its own horizontal padding
//!   inside the body slot. **Only when no footer is present**, the body's
//!   bottom edge additionally gains the window's bottom safe-area inset, so
//!   scrolled content never sits under a gesture-navigation bar; when a
//!   footer *is* present, the footer (not the body) absorbs the bottom inset
//!   (below). Scrolling is otherwise the caller's own concern: a body taller
//!   than the space it is given is clipped, not scrolled, so wrap genuinely
//!   tall content in a scroll view before handing it over (the same overflow
//!   contract [`crate::sheet`]'s content slot documents).
//! * **Footer** — present only when [`footer`](GlyphSideSheetView::footer) is
//!   set: a top `--border` hairline plus the caller's view, inset
//!   horizontally by the same [`BODY_PAD_X`] gutter the body uses and
//!   vertically by `FOOTER_PAD`, pinned to the panel's bottom edge and taking
//!   its height off the body's. Its *bottom* padding grows by the window's
//!   bottom safe-area inset, so the footer's content clears a gesture bar the
//!   same way the body does when there is no footer.
//!
//! # The close affordance is chrome this widget paints and handles
//!
//! The header's trailing edge carries a small circular press target this
//! widget paints itself — a [`CLOSE_DIAMETER`] chip filled one step above the
//! panel's own raised surface, holding a two-stroke `×` in dim ink that goes
//! accent while pressed (a painted vector, never a font glyph, so it does not
//! depend on font fallback). Its press fires the **same** staged exit the
//! scrim tap does, fired on up-inside like every other press in the catalog.
//!
//! The painted chip is the mock's own 26px, but its *hit* rect is inflated to
//! the catalog's 44px minimum touch target ([`crate::sheet`]'s
//! `HANDLE_TOUCH_TARGET` precedent); the inflated rect still sits entirely
//! inside the header row.
//!
//! # No drag-to-close, no snap points (v1)
//!
//! Deliberate: the approved design has neither, and a side sheet is not the
//! strongly drag-affording shape a bottom sheet is (there is no handle to
//! grab, and a horizontal drag inside the panel competes with the body's own
//! content). [`crate::sheet`]'s drag machinery is therefore **not** ported
//! here — adding it later is additive, not a rework.
//!
//! # `dismissable(bool)` + back-dismiss
//!
//! [`GlyphSideSheetView::dismissable`] (default `true`) is the single barrier
//! flag gating the scrim tap, `Escape`, an Android back press — **and the
//! close affordance, which a non-dismissable sheet does not paint at all**: an
//! affordance that does nothing is a lie, so the header simply reclaims its
//! trailing reserve for the title. The barrier still swallows presses (they
//! must not reach the page under the modal); it just arms no dismissal.
//!
//! `false` pushes [`BackPolicy::Veto`](frust::BackPolicy::Veto); `true`
//! pushes [`BackPolicy::DismissAnimated`](frust::BackPolicy::DismissAnimated)
//! and hands the widget the shared dismiss-signal cell
//! [`NavigatorController::request_back`] bumps, which `paint` compares against
//! the last-seen value to start the identical staged exit.
//!
//! **The flag is read once, at push time.** [`show_glyph_side_sheet`] peeks it
//! off one `build()` call to fix that page's `BackPolicy` and dismiss-signal
//! wiring for the page's whole life, and then **binds that same peeked value
//! onto every later build of the page**, so the widget cannot be built from
//! any other value; it likewise keeps the value it was *built* with when the
//! page is merely diffed. Flipping [`dismissable`](GlyphSideSheetView::dismissable)
//! on a later rebuild is **inert** — no relayout, the close affordance neither
//! appears nor disappears, and every dismiss guard keeps answering to the
//! pushed value. That is what keeps widget and host page agreeing: honouring a
//! post-push flip against a frozen policy would give a sheet that paints no
//! close affordance yet still back-dismisses, or one whose back press stays
//! vetoed forever. Honouring a live flip *properly* is future work — it needs
//! the pushed `BackPolicy` to become re-writable and the dismiss signal to grow
//! a second, policy-aware writer, which is additive rather than a rework.
//!
//! # Programmatic close — [`GlyphSideSheetHandle`], the same staged exit
//!
//! [`show_glyph_side_sheet`] returns a cheap cloneable handle whose
//! [`close`](GlyphSideSheetHandle::close) bumps that very cell, so an app-side
//! close (a footer "Apply" button) plays the identical scrim-fade + panel-slide
//! exit and pops on completion — where a bare `controller.pop()` tears the page
//! down mid-motion. Exactly-once by construction: repeat closes, or one landing
//! while another dismissal is already exiting, are absorbed by `begin_exit`'s
//! idempotence and the terminal `Dismissed` phase. A close requested before the
//! pushed page has painted once is honoured too (the widget starts at
//! generation 0, never at the cell's current value). A programmatic close
//! ignores [`dismissable(false)`](GlyphSideSheetView::dismissable): that flag
//! is a barrier against the *user*, and a sheet the app cannot close is a stuck
//! app.
//!
//! # The dismiss callback is state-free
//!
//! Because it fires from **paint** (after the exit animation),
//! [`on_close`](GlyphSideSheetView::on_close) is a plain `Fn()`, not a
//! `Fn(&mut State)` — the same shape [`crate::sheet`]/[`crate::dialog`] carry.
//! App state changes ride the navigator's `on_result` instead, delivered with
//! `&mut State` after the pop. [`show_glyph_side_sheet`] *composes* rather than
//! replaces: the pop is enqueued first, the caller's callback runs second, so
//! any navigator op the callback issues queues behind the sheet's own pop.
//!
//! # Semantics — `Role::Dialog` with the modal flag, labelled by the title
//!
//! One [`Role::Dialog`] container node with the accesskit **modal** flag and
//! the title as its label (matching [`crate::dialog`]'s labelled dialog node),
//! with the title, the close affordance's own `Role::Button` node, the body
//! subtree, and the footer subtree as its children.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{FontWeight, LineHeight, TextOverflow};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, ThemeTextType,
    View, Widget, any,
};
use frust::text;
use frust::{Curve, FrameTime};
use kurbo::{Affine, BezPath, Point, Rect, Size};
use peniko::{Brush, Color};

use frust::Timing;
use frust::{BackPolicy, NavigatorController, PopResult, PushOptions};
use frust::{TransitionDriver, TransitionSpec, make_driver};

use crate::press::presses;

/// Scrim opacity behind the panel at full enter — the Glyph modal-barrier
/// constant [`crate::dialog`]/[`crate::sheet`] both use, applied here too so
/// the catalog's modal surfaces carry one barrier weight.
const SCRIM_ALPHA: f32 = 0.6;
/// Scrim fade fraction at or below which the barrier counts as **gone**: it
/// paints nothing a user can see, so it must neither swallow input nor keep
/// frames coming. `docs/REVIEW_FOCUS.md`'s overlay rule — *a barrier gates its
/// input swallow on progress, never on open/closed state alone* — an invisible
/// surface that still absorbs pointers being the named defect. Small enough
/// that a real fade (which starts moving within one frame) is never mistaken
/// for a dead one.
const BARRIER_EPSILON: f32 = 1e-3;

/// The panel's width as a fraction of the viewport's, from the approved Glyph
/// side-sheet design: wide enough to be a real secondary surface, narrow
/// enough to leave the page under it legible as context.
const WIDTH_FRACTION: f64 = 0.83;
/// The panel's width ceiling, in logical px — the fraction above is what binds
/// on a phone, this on anything wider (the design's own figure; deliberately
/// tighter than M3's 320dp side sheet, matching Glyph's denser monospace
/// chrome).
///
/// Public so an app can size a companion layout against the panel, and named
/// in full because this crate's root re-exports every module flat.
pub const GLYPH_SIDE_SHEET_MAX_WIDTH: f64 = 284.0;

/// The panel's leading-edge hairline width, in logical px (`--border` = 1px).
const BORDER_WIDTH: f64 = 1.0;
/// Header/footer divider thickness, in logical px — the same hairline.
const HAIRLINE: f64 = 1.0;

/// Header inset on the leading edge, in logical px. Glyph ships no spacing
/// *token* scale (unlike `frust_material`'s `MaterialSpacing`), so — like
/// [`crate::dialog`]'s `DIALOG_PADDING` family — the design's own numbers
/// stand as named constants: 16 leading / 16 top / 14 trailing / 13 bottom.
const HEADER_PAD_LEADING: f64 = 16.0;
/// Header inset on the trailing edge, before the close affordance.
const HEADER_PAD_TRAILING: f64 = 14.0;
/// Header inset above its content.
const HEADER_PAD_TOP: f64 = 16.0;
/// Header inset below its content (its bottom hairline sits inside this).
const HEADER_PAD_BOTTOM: f64 = 13.0;
/// Gap kept between the title's text box and the close chip, in logical px —
/// the catalog's gutter step ([`crate::dialog`]'s `TITLE_BODY_GAP`/
/// `ACTION_GAP` family).
const TITLE_CLOSE_GAP: f64 = 10.0;

/// The close affordance's painted chip diameter, in logical px (the design's
/// own figure).
const CLOSE_DIAMETER: f64 = 26.0;
/// The close affordance's hit-target edge, in logical px — iOS's 44pt minimum
/// touch target, the same figure [`crate::sheet`]'s drag strip uses, inflated
/// symmetrically around the smaller painted chip and still contained by the
/// header row's own height.
const CLOSE_TOUCH_TARGET: f64 = 44.0;
/// Half-span of each arm of the painted `×`, in logical px (a 10px glyph
/// inside the 26px chip).
const CLOSE_ARM: f64 = 5.0;
/// Stroke width of the painted `×`, in logical px (mirrors
/// [`crate::appbar`]'s own widget-painted close glyph).
const CLOSE_STROKE_WIDTH: f64 = 1.5;

/// The header row's fixed height, in logical px: its two vertical insets
/// around the close chip, which is the row's tallest element (taller than the
/// title's own line height). Crate-private deliberately — the root re-exports
/// this module flat, and a bare `HEADER_HEIGHT` in that namespace would be a
/// name the next component to want one cannot have.
const HEADER_HEIGHT: f64 = HEADER_PAD_TOP + CLOSE_DIAMETER + HEADER_PAD_BOTTOM;

/// The footer's *vertical* inset (top gap before its content, and its own
/// bottom gap before growing by the window's bottom safe-area inset — see
/// [`GlyphSideSheetWidget::layout`]), in logical px — the header's own
/// leading inset. The footer's *horizontal* inset is [`BODY_PAD_X`] instead,
/// so it shares the body's gutter (see the [module docs](self)'s
/// Header/body/footer section).
const FOOTER_PAD: f64 = HEADER_PAD_LEADING;

/// The body (and footer) slot's horizontal gutter, in logical px, on *both*
/// edges — the approved mock's `.side-sheet-body` padding (12px sides; the
/// same rule's 18px bottom figure is not reproduced here as a static value —
/// the body's bottom edge instead gains exactly the window's bottom
/// safe-area inset when there is no footer, see the [module docs](self)).
/// Glyph ships no spacing *token* scale (unlike `frust_material`'s
/// `MaterialSpacing`), so — like [`HEADER_PAD_LEADING`]'s family — this is a
/// named constant carrying the design's own figure. The component's own
/// inset: a caller must not re-add horizontal padding inside the body slot
/// (see the [module docs](self)).
const BODY_PAD_X: f64 = 12.0;

/// Title type token (15/700 — [`crate::dialog`]'s title token, since a side
/// sheet's header title is the same class of surface title). Size, weight and
/// line height are hardcoded rather than read from a live
/// `Theme::type_scale`: `Text` resolves only a *color* role and an opt-in
/// family role after `View::build` (see `crates/frust-widgets/src/text.rs`'s
/// `effective_style`), the precedent every other titled Glyph surface takes.
/// The family is that opt-in role: `headlineSmall`.
const TITLE_SIZE: f32 = 15.0;
const TITLE_WEIGHT: FontWeight = FontWeight::BOLD;
const TITLE_LINE_HEIGHT: f32 = 20.0;

/// Unthemed-fallback panel surface fill (Glyph dark `bg-raised` `#1e2430`,
/// matching [`crate::sheet`]'s panel — a side sheet is the same floating
/// surface over the page). A theme resolves this from
/// `colors.surface_container_high`.
const CONTAINER: Color = Color::from_rgb8(0x1e, 0x24, 0x30);
/// Unthemed-fallback leading-edge hairline (Glyph dark `border-bright`). A
/// theme resolves this from `colors.outline`.
const BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Unthemed-fallback header/footer divider (Glyph dark `--border`). A theme
/// resolves this from `colors.outline_variant` — the divider role every other
/// Glyph container splits its rows with (`crate::list`, `crate::accordion`,
/// `crate::tabs`), one step quieter than the panel's own outer hairline.
const DIVIDER: Color = Color::from_rgb8(0x2a, 0x2d, 0x33);
/// Unthemed-fallback scrim base color (a theme resolves this from
/// `colors.scrim`).
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback close-chip fill (Glyph dark `bg-overlay` `#272d3d`). A
/// theme resolves this from `colors.surface_container_highest` — the design
/// calls for a *raised* chip, and the panel it sits on is already `bg-raised`,
/// so the chip takes the next step up rather than vanishing into its own
/// backdrop.
const CLOSE_FILL: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);
/// Unthemed-fallback close-glyph ink (Glyph dark `fg-muted` `#a39c88`). A
/// theme resolves this from `colors.on_surface_variant` — the catalog's
/// documented position for the design's dim ink, which has no `ColorScheme`
/// role of its own.
const CLOSE_INK: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
/// Unthemed-fallback pressed close-glyph ink (Glyph dark accent amber). A
/// theme resolves this from `colors.primary` — the accent **text/icon** role,
/// never `primary_container` (the bright fill).
const CLOSE_INK_PRESSED: Color = Color::from_rgb8(0xff, 0xb6, 0x27);

/// Enter duration fallback (`durations.base` = 220ms) when no theme is
/// threaded — mirrors [`crate::sheet`].
const ENTER_DURATION: Duration = Duration::from_millis(220);
/// Enter easing fallback (Glyph `spatial` cubic).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration fallback (`durations.fast` = 150ms; exits faster than
/// entrances).
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

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state —
/// identical resolution to [`crate::sheet`]'s, kept as its own copy because
/// the modal modules in this catalog are intentionally not coupled.
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

fn resolve_divider(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().outline_variant,
        None => DIVIDER,
    }
}

fn resolve_scrim(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().scrim,
        None => SCRIM,
    }
}

fn resolve_close_fill(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_highest,
        None => CLOSE_FILL,
    }
}

/// The close glyph's ink for the current press state — dim at rest, accent
/// while pressed.
fn resolve_close_ink(theme: Option<&Theme>, pressed: bool) -> Color {
    match (theme, pressed) {
        (Some(t), false) => t.scheme().on_surface_variant,
        (Some(t), true) => t.scheme().primary,
        (None, false) => CLOSE_INK,
        (None, true) => CLOSE_INK_PRESSED,
    }
}

/// Coerce a possibly-infinite constraint dimension to a finite value (a side
/// sheet expects bounded constraints — a navigator page or a full-screen
/// `Stack`).
fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

/// The panel width for a viewport `width` logical px wide.
fn panel_width(width: f64) -> f64 {
    (WIDTH_FRACTION * width).clamp(0.0, GLYPH_SIDE_SHEET_MAX_WIDTH)
}

/// The header title's child view: single-line, ellipsized — the header's
/// height is fixed, so a long title must shorten rather than wrap out of it.
/// Its family is the theme's `headlineSmall` role, resolved at layout.
fn title_view<State: 'static>(s: &str) -> AnyView<State> {
    any::<State, _>(
        text(s.to_string())
            .size(TITLE_SIZE)
            .weight(TITLE_WEIGHT)
            .line_height(LineHeight::Absolute(TITLE_LINE_HEIGHT))
            .max_lines(1)
            .overflow(TextOverflow::Ellipsis)
            .themed_family(ThemeTextType::HeadlineSmall),
    )
}

/// A state-free "close this sheet" callback — see the [module docs](self)'s
/// "The dismiss callback is state-free" note.
type OnClose = Rc<dyn Fn()>;

/// A declarative Glyph modal side sheet. See the [module docs](self).
pub struct GlyphSideSheetView<State: 'static> {
    title: String,
    body: AnyView<State>,
    footer: Option<AnyView<State>>,
    on_close: Option<OnClose>,
    dismissable: bool,
    /// The shared dismiss-signal cell (the `DismissAnimated` back-press seam,
    /// and [`GlyphSideSheetHandle`]'s programmatic close) — wired internally
    /// by [`show_glyph_side_sheet`], never part of the public builder surface.
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

/// Wrap `body` in a Glyph modal side sheet titled `title`. Chain
/// [`footer`](GlyphSideSheetView::footer) for a sticky footer,
/// [`dismissable`](GlyphSideSheetView::dismissable) to change the dismiss
/// barrier, and [`on_close`](GlyphSideSheetView::on_close) to wire dismissal
/// (usually `controller.pop()` — [`show_glyph_side_sheet`] does this for you).
pub fn glyph_side_sheet<State: 'static, V: View<State>>(
    title: impl Into<String>,
    body: V,
) -> GlyphSideSheetView<State> {
    GlyphSideSheetView {
        title: title.into(),
        body: any(body),
        footer: None,
        on_close: None,
        dismissable: true,
        dismiss_signal: None,
    }
}

/// PascalCase alias for [`glyph_side_sheet`], matching the widget-fn
/// vocabulary.
#[allow(non_snake_case)]
pub fn GlyphSideSheet<State: 'static, V: View<State>>(
    title: impl Into<String>,
    body: V,
) -> GlyphSideSheetView<State> {
    glyph_side_sheet(title, body)
}

impl<State: 'static> GlyphSideSheetView<State> {
    /// Pin `footer` to the panel's bottom edge, above a top hairline divider
    /// and inset on every side — a sticky action row that never scrolls with
    /// the body. Absent by default; the body takes the whole space below the
    /// header without one.
    ///
    /// Takes any view and erases it here. Erasure is idempotent, so a caller
    /// composing its footer elsewhere as an [`AnyView`] does not pay a second
    /// erasure.
    pub fn footer(mut self, footer: impl View<State>) -> Self {
        self.footer = Some(AnyView::new(footer));
        self
    }

    /// Whether this sheet can be dismissed by the user at all — the scrim tap,
    /// the close affordance, `Escape`, and an Android back press (default
    /// `true`). `false` disables all four **and paints no close affordance**
    /// (see the [module docs](self)); the app still closes it itself, through
    /// [`GlyphSideSheetHandle::close`] (staged) or a bare `controller.pop()`
    /// (immediate).
    ///
    /// **Read at push time only.** [`show_glyph_side_sheet`] fixes the pushed
    /// page's [`BackPolicy`] from this flag, so the widget holds the value it
    /// was built with and flipping it on a later rebuild is deliberately inert
    /// (see the [module docs](self)) — decide it when the sheet is shown.
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the state-free close callback — invoked once, from paint, when the
    /// exit animation completes after any dismissal (a scrim tap, the close
    /// affordance, `Escape`, a back press, or
    /// [`GlyphSideSheetHandle::close`]). [`show_glyph_side_sheet`] *composes*
    /// `controller.pop()` with this rather than replacing it: the pop is
    /// enqueued first, then this callback runs.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }
}

/// A programmatic close handle for one side sheet pushed by
/// [`show_glyph_side_sheet`] — the app-side seam onto the widget's own staged
/// exit (see the [module docs](self)'s "Programmatic close").
///
/// Cheap and cloneable (one `Rc` cell, no `State` parameter, so a sheet body
/// can pass it around freely). Every clone drives the same sheet, and closing
/// it more than once — or while a user dismissal is already exiting — still
/// yields exactly one exit and one pop.
#[derive(Clone, Debug)]
pub struct GlyphSideSheetHandle {
    /// The pushed page's dismiss-signal cell: the identical generation counter
    /// [`NavigatorController::request_back`] bumps, observed by the widget's
    /// `paint`.
    signal: Rc<Cell<u64>>,
}

impl GlyphSideSheetHandle {
    /// Close the sheet the way the user's own gestures close it: begin the
    /// staged exit (scrim fade + panel slide over `durations.fast`) and pop
    /// the page when it completes — never an immediate pop.
    ///
    /// Idempotent, and inert once the sheet is exiting or gone. Closes a
    /// [`dismissable(false)`](GlyphSideSheetView::dismissable) sheet too: that
    /// flag gates *user* dismissal, and this is the app's own act.
    pub fn close(&self) {
        self.signal.set(self.signal.get().wrapping_add(1));
    }
}

/// Push `build`'s side sheet as a transparent navigator page (the page below
/// stays visible under the scrim), register `on_result` for the value the
/// sheet pops with, and return its [`GlyphSideSheetHandle`] for a programmatic
/// close.
///
/// The scrim tap / close affordance / `Escape` / back press are wired to
/// `controller.pop()` (an empty [`PopResult`]) **composed** with any
/// [`on_close`](GlyphSideSheetView::on_close) `build`'s view already carries:
/// on the one exit completion the pop is enqueued first, then the caller's
/// callback runs, each exactly once.
///
/// The sheet is pushed with [`TransitionSpec::NONE`] — the navigator supplies
/// the modal contract, the widget supplies its own scrim-fade + panel-slide
/// staging (see the [module docs](self)).
///
/// ```ignore
/// let sheet = show_glyph_side_sheet(
///     &state.nav,
///     || glyph_side_sheet("Filters", filter_form()),
///     |state: &mut State, result: PopResult| { /* ... */ },
/// );
/// // …later, from a footer button that applies and dismisses:
/// sheet.close();
/// ```
pub fn show_glyph_side_sheet<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) -> GlyphSideSheetHandle
where
    State: 'static,
    B: Fn() -> GlyphSideSheetView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    // Peeked once, at show-time: the back-policy/dismiss-signal wiring is
    // fixed for the life of this pushed page (mirrors
    // `crate::sheet::show_glyph_sheet`'s identical peek), even though `build`
    // is re-invoked on every later navigator rebuild to diff the page. This
    // value is then bound back onto each of those builds (below), and the
    // widget ignores the flag when it is merely diffed (see
    // `GlyphSideSheetView::rebuild`), so page and panel can never disagree
    // about whether this sheet is dismissable.
    let dismissable = build().dismissable;
    // One cell, two writers: the navigator's back press (only under
    // `DismissAnimated`) and the returned handle. The widget always gets it —
    // a non-dismissable sheet still owes the app a staged close — but the
    // navigator only gets it when a back press may legitimately dismiss.
    let signal = Rc::new(Cell::new(0u64));
    let widget_signal = signal.clone();
    let mut options = PushOptions::transparent()
        .transition(TransitionSpec::NONE)
        .back(if dismissable {
            BackPolicy::DismissAnimated
        } else {
            BackPolicy::Veto
        })
        .on_result(on_result);
    if dismissable {
        options = options.dismiss_signal(signal.clone());
    }
    controller.push_with_options(
        move || {
            let ctrl = close_ctrl.clone();
            let mut view = build();
            // Compose, never clobber: the caller's own `on_close` survives the
            // wiring, and runs after the pop it is composed with.
            let caller_close = view.on_close.take();
            let mut view = view.on_close(move || {
                ctrl.pop();
                if let Some(on_close) = &caller_close {
                    on_close();
                }
            });
            view.dismiss_signal = Some(widget_signal.clone());
            // Bind the *peeked* flag onto every build of this page, so the
            // widget can only ever be built from the value the page's
            // `BackPolicy` was fixed from. `build` is re-invoked on every
            // navigator rebuild, and a flip landing between this call and the
            // first of those rebuilds — or any later fresh mount — would
            // otherwise build a widget disagreeing with the pushed policy.
            // With this line the push-time contract is structural, not a
            // convention `rebuild`'s deliberate non-reinstall enforces only
            // for a widget that already exists.
            view.dismissable = dismissable;
            any::<State, _>(view)
        },
        options,
    );
    GlyphSideSheetHandle { signal }
}

/// The sheet's enter/exit lifecycle phase (mirrors [`crate::sheet`]'s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Animating in (panel sliding in from the trailing edge, scrim fading in).
    Enter,
    /// Fully shown, at rest.
    Shown,
    /// Animating out after a scrim tap / close affordance / `Escape` / back
    /// press or a [`GlyphSideSheetHandle::close`].
    Exit,
    /// Exit complete; the close callback has fired and the page will be popped.
    Dismissed,
}

/// Reconcile the optional footer slot in place — born, dying, or diffed.
fn reconcile_footer<State: 'static>(
    prev: Option<&AnyView<State>>,
    next: Option<&AnyView<State>>,
    pod: &mut Option<ChildPod>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (prev, next) {
        (None, Some(v)) => {
            *pod = Some(frust::authoring::build_child(v, ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(v), None) => {
            if let Some(p) = pod.as_mut() {
                frust::authoring::teardown_child(v, p, ctx);
            }
            *pod = None;
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (Some(a), Some(b)) => match pod.as_mut() {
            Some(p) => frust::authoring::rebuild_child(a, b, p, ctx),
            None => ChangeFlags::NONE,
        },
        (None, None) => ChangeFlags::NONE,
    }
}

/// The retained widget for a [`GlyphSideSheetView`]. See the
/// [module docs](self).
pub struct GlyphSideSheetWidget {
    title: ChildPod,
    /// The title string, kept for the semantics node's label.
    title_text: String,
    body: ChildPod,
    footer: Option<ChildPod>,
    on_close: Option<OnClose>,
    /// The user-dismiss barrier flag, **as of `build`** — the push-time
    /// contract every guard in this widget reads (see the [module docs](self));
    /// `rebuild` deliberately never reinstalls it.
    dismissable: bool,
    /// The shared dismiss-signal cell — the one channel carrying **both** an
    /// Android back press (the `DismissAnimated` seam) and a
    /// [`GlyphSideSheetHandle::close`]; see
    /// [`observe_dismiss_signal`](Self::observe_dismiss_signal).
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last generation observed from `dismiss_signal`. Starts at 0, never
    /// at the cell's build-time value: [`show_glyph_side_sheet`] mints the cell
    /// fresh per push, so a non-zero generation when this widget builds is a
    /// close requested before the page mounted — swallowing it would strand a
    /// sheet the app has already closed on screen.
    last_seen_dismiss: u64,
    /// The trailing-edge panel rect **at rest** (fully shown), in the widget's
    /// own local coordinate space — used both to lay the panel out and,
    /// deliberately unanimated, as the hit-test region for the scrim/panel
    /// split (the offset is a paint-time transform, never a layout change).
    panel: Rect,
    /// The body's clip/extent rect (local coords, at rest).
    body_rect: Rect,
    /// The close affordance's hit rect (local coords, at rest) —
    /// [`Rect::ZERO`] on a non-dismissable sheet, which paints no affordance
    /// at all.
    close_rect: Rect,
    /// The painted close chip's center (local coords, at rest).
    close_center: Point,
    /// The footer band's top hairline y (local coords, at rest); meaningful
    /// only while `footer` is `Some`.
    footer_divider_y: f64,
    /// The window's top safe-area inset, as last observed by `layout` — the
    /// header band grows by this (see the [module docs](self)'s
    /// Header/body/footer section). Kept for tests; the panel itself stays
    /// full-bleed regardless.
    top_inset: f64,
    /// The header band's total height (`top_inset + HEADER_HEIGHT`), as last
    /// laid out — where the body starts and the header's own bottom hairline
    /// sits.
    header_height: f64,
    phase: Phase,
    /// The current animation's progress driver — enter or exit, whichever the
    /// phase says is running. Lazily built on the first paint that sees it,
    /// because rebuild has no theme to resolve a [`Timing`] from.
    driver: Option<TransitionDriver>,
    /// The panel offset fraction the in-flight exit started from — `0.0` for
    /// every dismissal off a resting sheet, the entering panel's own offset
    /// for one dismissed mid-enter, so an exit never flashes the panel back to
    /// rest before sliding it out.
    exit_from: f64,
    /// The panel's offset fraction as of the last [`advance`](Self::advance).
    offset_frac: f64,
    /// The close affordance is showing its pressed ink.
    close_pressed: bool,
    /// A close-affordance press is in flight (it captured the pointer).
    close_captured: bool,
    /// A scrim/panel-background press is in flight (the modal barrier captured
    /// the pointer).
    scrim_captured: bool,
    /// Whether that press started outside the panel (only an outside press
    /// released outside dismisses).
    scrim_down_outside: bool,
}

impl<State: 'static> View<State> for GlyphSideSheetView<State> {
    type Element = GlyphSideSheetWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GlyphSideSheetWidget {
        GlyphSideSheetWidget {
            title: frust::authoring::build_child(&title_view::<State>(&self.title), ctx),
            title_text: self.title.clone(),
            body: frust::authoring::build_child(&self.body, ctx),
            footer: self
                .footer
                .as_ref()
                .map(|v| frust::authoring::build_child(v, ctx)),
            on_close: self.on_close.clone(),
            dismissable: self.dismissable,
            dismiss_signal: self.dismiss_signal.clone(),
            last_seen_dismiss: 0,
            panel: Rect::ZERO,
            body_rect: Rect::ZERO,
            close_rect: Rect::ZERO,
            close_center: Point::ZERO,
            footer_divider_y: 0.0,
            top_inset: 0.0,
            header_height: HEADER_HEIGHT,
            phase: Phase::Enter,
            driver: None,
            exit_from: 0.0,
            // A freshly built sheet has not painted yet: it sits fully
            // off-screen past the trailing edge, exactly where its enter
            // starts.
            offset_frac: 1.0,
            close_pressed: false,
            close_captured: false,
            scrim_captured: false,
            scrim_down_outside: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphSideSheetWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_child(
            &title_view::<State>(&prev.title),
            &title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        element.title_text = self.title.clone();
        flags |= frust::authoring::rebuild_child(&prev.body, &self.body, &mut element.body, ctx);
        flags |= reconcile_footer(
            prev.footer.as_ref(),
            self.footer.as_ref(),
            &mut element.footer,
            ctx,
        );
        // Closures aren't comparable; reinstall the close adapter unconditionally.
        element.on_close = self.on_close.clone();
        // `dismissable` is deliberately NOT reinstalled — it is a *push-time*
        // contract (see the module docs). `show_glyph_side_sheet` freezes the
        // pushed page's `BackPolicy` and dismiss-signal wiring off the flag it
        // peeks once, so a post-push flip honoured here could only desynchronize
        // the two: a sheet painting no close affordance that still
        // back-dismisses, or one whose back press stays vetoed forever. The
        // widget keeps its build-time value, which is what makes the flip
        // genuinely inert — no relayout, no repaint, and no guard to re-read it.
        // The dismiss-signal cell's identity is fixed at push time (see
        // `show_glyph_side_sheet`); reinstalling it here never disturbs
        // `last_seen_dismiss`.
        element.dismiss_signal = self.dismiss_signal.clone();
        flags
    }

    fn teardown(&self, element: &mut GlyphSideSheetWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(
            &title_view::<State>(&self.title),
            &mut element.title,
            ctx,
        );
        frust::authoring::teardown_child(&self.body, &mut element.body, ctx);
        if let (Some(v), Some(p)) = (&self.footer, element.footer.as_mut()) {
            frust::authoring::teardown_child(v, p, ctx);
        }
    }
}

impl GlyphSideSheetWidget {
    /// Begin the exit animation, from wherever the panel currently sits —
    /// mid-enter, never snapped back to rest first.
    ///
    /// Idempotent: only an `Enter`/`Shown` sheet can start exiting, so a second
    /// request mid-exit — or after `Dismissed` — is a no-op. **Every** close
    /// path (scrim, close affordance, `Escape`, back, and
    /// [`GlyphSideSheetHandle::close`]) funnels through this one function.
    fn begin_exit(&mut self) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.phase = Phase::Exit;
            self.exit_from = self.offset_frac.clamp(0.0, 1.0);
            self.driver = None;
        }
    }

    /// Observe the shared dismiss-signal cell and begin the exit staging
    /// exactly once per bump — whether the bump came from a back press or from
    /// [`GlyphSideSheetHandle::close`] (mirrors [`crate::sheet`]'s). A back
    /// request flags `PAINT`, so this always runs before the next frame is
    /// shown; a handle close rides the app's own rebuild.
    ///
    /// Deliberately **not** gated on `dismissable`: the navigator only routes a
    /// back press here for a dismissable sheet in the first place (a
    /// non-dismissable one pushes [`BackPolicy::Veto`]), so the only writer
    /// left is the app's own handle, which closes either kind. That invariant
    /// holds because the flag is a *push-time* contract — the pushed
    /// [`BackPolicy`] and this widget's own `dismissable` are fixed from the
    /// same peeked value and a later flip is ignored (see
    /// [`GlyphSideSheetView::rebuild`](GlyphSideSheetView) and the
    /// [module docs](self)) — so the policy can never drift away from the
    /// barrier this sheet actually enforces.
    fn observe_dismiss_signal(&mut self) {
        if let Some(signal) = &self.dismiss_signal {
            let current = signal.get();
            if current != self.last_seen_dismiss {
                self.last_seen_dismiss = current;
                self.begin_exit();
            }
        }
    }

    /// Advance the enter/exit phase machine to frame time `now`, returning
    /// `(offset_frac, scrim_frac, animating)`: `offset_frac` is how far the
    /// panel sits past its rest position toward the trailing edge, as a
    /// fraction of its own width (`0.0` = at rest, `1.0` = fully off-screen);
    /// `scrim_frac` is the scrim's fade fraction, `1 − offset_frac`, so the
    /// barrier's weight always reads how far the panel has travelled. (The
    /// scrim still fades *independently of the panel's transform*: it is
    /// painted before it, and never moves.) Factored out of `paint` so the full
    /// timeline is drivable with synthetic [`FrameTime`]s in a unit test.
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f32, bool) {
        let (offset, animating) = match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                (1.0 - adv.value.clamp(0.0, 1.0), !adv.done)
            }
            Phase::Shown => (0.0, false),
            Phase::Exit => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let q = adv.value.clamp(0.0, 1.0);
                if adv.done {
                    self.phase = Phase::Dismissed;
                    self.driver = None;
                    if let Some(on_close) = &self.on_close {
                        on_close();
                    }
                }
                // The driver's `0 → 1` remapped onto `exit_from → 1`: a
                // dismissal mid-enter resumes from where the panel had reached.
                (self.exit_from + (1.0 - self.exit_from) * q, !adv.done)
            }
            Phase::Dismissed => (1.0, false),
        };
        self.offset_frac = offset;
        (offset, (1.0 - offset) as f32, animating)
    }

    /// Whether the modal barrier is actually on screen, and so may swallow the
    /// input it is there to keep off the page below: the scrim's fade fraction
    /// as of the last [`advance`](Self::advance) (`1 − offset_frac`, the value
    /// `paint` fills it with) against [`BARRIER_EPSILON`].
    ///
    /// `false` past a completed dismissal — `Phase::Dismissed` is terminal, and
    /// a standalone-composed sheet (a bare [`glyph_side_sheet`] with no
    /// navigator page to unmount it) simply stays mounted there. Gating on the
    /// progress rather than on the phase is `docs/REVIEW_FOCUS.md`'s overlay
    /// rule; it also, for the one still-transparent frame at the head of an
    /// entrance, lets a press through to the page the sheet has not yet covered.
    fn barrier_is_live(&self) -> bool {
        (1.0 - self.offset_frac) as f32 > BARRIER_EPSILON
    }

    /// Whether the focus path runs into this sheet's own content — the guard
    /// `Escape` consults, so content holding the deeper focus path gets first
    /// crack at the key.
    fn content_is_focused(&self) -> bool {
        self.body.is_focused() || self.footer.as_ref().is_some_and(|p| p.is_focused())
    }

    /// Route a non-barrier event to the body, then the footer.
    fn route_to_content(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if frust::authoring::route_event_single(&mut self.body, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        match self.footer.as_mut() {
            Some(pod) => frust::authoring::route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    /// Paint the close affordance: a filled chip holding a two-stroke `×`
    /// (chrome, never a font glyph — no font-fallback dependency), tinted dim
    /// or accent by its press state.
    ///
    /// Takes its two colors already resolved rather than a `&Theme`, because
    /// `paint` must finish reading the theme out of its `PaintCtx` before it
    /// starts handing that same ctx to its children.
    fn paint_close(&self, origin: Point, fill: Color, ink: Color, scene: &mut dyn PaintScene) {
        let cx = origin.x + self.close_center.x;
        let cy = origin.y + self.close_center.y;
        scene.fill_rounded_rect(
            Point::new(cx - CLOSE_DIAMETER / 2.0, cy - CLOSE_DIAMETER / 2.0),
            Size::new(CLOSE_DIAMETER, CLOSE_DIAMETER),
            CLOSE_DIAMETER / 2.0,
            fill,
        );
        let mut a = BezPath::new();
        a.move_to((cx - CLOSE_ARM, cy - CLOSE_ARM));
        a.line_to((cx + CLOSE_ARM, cy + CLOSE_ARM));
        let mut b = BezPath::new();
        b.move_to((cx + CLOSE_ARM, cy - CLOSE_ARM));
        b.line_to((cx - CLOSE_ARM, cy + CLOSE_ARM));
        scene.stroke_path(Point::ZERO, &a, CLOSE_STROKE_WIDTH, &Brush::Solid(ink));
        scene.stroke_path(Point::ZERO, &b, CLOSE_STROKE_WIDTH, &Brush::Solid(ink));
    }
}

impl Widget for GlyphSideSheetWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        // Top/bottom safe-area insets — zero when no shell has pushed any
        // (headless tests, a desktop preview without a window-inset source),
        // exactly like `crate::appbar`'s identical read. The **panel** stays
        // full-bleed (painted edge to edge under the status bar, below); only
        // the header's own *content* — and the body/footer bottom padding —
        // shift for these.
        let insets = ctx.window_insets().padding();
        let top_inset = insets.top;
        let bottom_inset = insets.bottom;
        self.top_inset = top_inset;

        let panel_w = panel_width(area_w);
        let panel_x = area_w - panel_w;
        self.panel = Rect::new(panel_x, 0.0, area_w, area_h);

        // Header: the title, leading-inset and vertically centered in the
        // fixed HEADER_HEIGHT row — offset below `top_inset` — with the close
        // affordance's own footprint kept clear on the trailing edge —
        // reclaimed for the title when this sheet paints no affordance at
        // all.
        let header_height = top_inset + HEADER_HEIGHT;
        self.header_height = header_height;
        let trailing_reserve = if self.dismissable {
            HEADER_PAD_TRAILING + CLOSE_DIAMETER + TITLE_CLOSE_GAP
        } else {
            HEADER_PAD_TRAILING
        };
        let title_max_w = (panel_w - HEADER_PAD_LEADING - trailing_reserve).max(0.0);
        let title_size = self.title.layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(title_max_w, f64::INFINITY)),
        );
        self.title.set_origin(Point::new(
            panel_x + HEADER_PAD_LEADING,
            top_inset + ((HEADER_HEIGHT - title_size.height) / 2.0).max(0.0),
        ));

        if self.dismissable {
            self.close_center = Point::new(
                area_w - HEADER_PAD_TRAILING - CLOSE_DIAMETER / 2.0,
                top_inset + HEADER_HEIGHT / 2.0,
            );
            self.close_rect = Rect::from_center_size(
                self.close_center,
                Size::new(CLOSE_TOUCH_TARGET, CLOSE_TOUCH_TARGET),
            );
        } else {
            self.close_center = Point::ZERO;
            self.close_rect = Rect::ZERO;
        }

        // Footer: hairline + the caller's view, inset horizontally by
        // BODY_PAD_X (the body's own gutter) and vertically by FOOTER_PAD,
        // hugging the panel's bottom — present only when one was supplied.
        // Its own bottom padding grows by `bottom_inset`, so its content
        // clears a gesture-navigation bar.
        let mut footer_h = 0.0;
        if let Some(pod) = self.footer.as_mut() {
            let footer_bottom_pad = FOOTER_PAD + bottom_inset;
            let inner_w = (panel_w - 2.0 * BODY_PAD_X).max(0.0);
            let max_h =
                (area_h - header_height - HAIRLINE - FOOTER_PAD - footer_bottom_pad).max(0.0);
            let size = pod.layout_child(ctx, &BoxConstraints::loose(Size::new(inner_w, max_h)));
            footer_h = HAIRLINE + FOOTER_PAD + size.height + footer_bottom_pad;
            let divider_y = area_h - footer_h;
            self.footer_divider_y = divider_y;
            pod.set_origin(Point::new(
                panel_x + BODY_PAD_X,
                divider_y + HAIRLINE + FOOTER_PAD,
            ));
        }

        // Body: everything between the header and the footer, tight on both
        // axes, inset horizontally by BODY_PAD_X on both edges. Its bottom
        // edge additionally gains `bottom_inset` when there is no footer to
        // absorb it instead (see the [module docs](self)). The clip/extent
        // rect (`body_rect`) still spans the panel's full width; only the
        // child's own geometry is inset.
        let body_h = (area_h - header_height - footer_h).max(0.0);
        let body_bottom_pad = if self.footer.is_some() {
            0.0
        } else {
            bottom_inset
        };
        let inner_body_w = (panel_w - 2.0 * BODY_PAD_X).max(0.0);
        let inner_body_h = (body_h - body_bottom_pad).max(0.0);
        self.body.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(inner_body_w, inner_body_h)),
        );
        self.body
            .set_origin(Point::new(panel_x + BODY_PAD_X, header_height));
        self.body_rect = Rect::new(panel_x, header_height, area_w, header_height + body_h);

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.observe_dismiss_signal();
        let theme = Theme::from_paint_ctx(ctx);
        let (enter, exit) = resolve_timings(theme);
        let was_dismissed = self.phase == Phase::Dismissed;
        let (offset_frac, scrim_frac, animating) = self.advance(ctx.frame_time(), enter, exit);
        let just_dismissed = !was_dismissed && self.phase == Phase::Dismissed;

        // Scrim fills the whole area (the modal barrier) — its own fade,
        // painted *before* any transform, so it never moves.
        let scrim = with_alpha(resolve_scrim(theme), SCRIM_ALPHA * scrim_frac);
        scene.fill_rect(ctx.origin(), ctx.size(), scrim);

        // The panel translates by up to its own width, straight out toward the
        // trailing edge — a pure slide, no scale, under one transform pushed
        // after the scrim.
        let panel_w = self.panel.width();
        scene.push_transform(Affine::translate((offset_frac * panel_w, 0.0)));

        let origin = ctx.origin();
        let panel_origin = Point::new(origin.x + self.panel.x0, origin.y + self.panel.y0);
        let panel_size = Size::new(panel_w, self.panel.height());
        // Square corners: the panel runs edge to edge (see the module docs).
        scene.fill_rect(panel_origin, panel_size, resolve_container(theme));
        scene.fill_rect(
            panel_origin,
            Size::new(BORDER_WIDTH, panel_size.height),
            resolve_border(theme),
        );

        let divider = resolve_divider(theme);
        let close_fill = resolve_close_fill(theme);
        let close_ink = resolve_close_ink(theme, self.close_pressed);
        // The header's bottom hairline sits inside its own total height
        // (`top_inset + HEADER_HEIGHT`), so the body starts exactly at
        // `self.header_height`.
        scene.fill_rect(
            Point::new(panel_origin.x, origin.y + self.header_height - HAIRLINE),
            Size::new(panel_w, HAIRLINE),
            divider,
        );
        self.title.paint_child(ctx, scene);
        if self.dismissable {
            self.paint_close(origin, close_fill, close_ink, scene);
        }

        // The body is the component's to clip: it was given a tight extent, and
        // anything the caller overflows it with stays inside the panel.
        scene.push_clip(
            Point::new(origin.x + self.body_rect.x0, origin.y + self.body_rect.y0),
            self.body_rect.size(),
        );
        self.body.paint_child(ctx, scene);
        scene.pop_clip();

        if let Some(pod) = self.footer.as_mut() {
            scene.fill_rect(
                Point::new(panel_origin.x, origin.y + self.footer_divider_y),
                Size::new(panel_w, HAIRLINE),
                divider,
            );
            pod.paint_child(ctx, scene);
        }
        scene.pop_transform();

        // Keep frames coming while the enter/exit is moving, and for the *one*
        // frame the exit lands on, so the rebuild that applies the pop actually
        // runs. At rest — shown, or dismissed and faded out — nothing at all
        // (idle-heat discipline).
        //
        // That landing frame is an **edge**, never `self.phase ==
        // Phase::Dismissed`: the phase is terminal, so testing it as a level
        // asks for a frame every frame, forever, in any composition that
        // outlives the dismissal (a standalone `glyph_side_sheet` with no
        // navigator page to unmount it). The barrier's own weight is not a
        // second condition here: a settled *shown* sheet has a fully opaque
        // scrim and still owes no frames — progress gates what the barrier
        // *swallows* (see `barrier_is_live`), while motion gates what it paints.
        //
        // DEFERRED: `crate::sheet` and `crate::dialog` still carry that exact
        // level test, together with the same "swallows input while invisible"
        // half in their own barrier arms; the catalog-wide fix is queued and is
        // deliberately not made here (this task's writable surface is this
        // module alone).
        if animating || just_dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // 0. A broadcast is not user input, so it belongs to none of the arms
        //    below: forward it to the content unconditionally and consume
        //    nothing, so a deferred callback queued inside the sheet still
        //    flushes while a finger is down.
        if event.is_broadcast() {
            frust::authoring::route_event_single(&mut self.body, ctx, event);
            if let Some(pod) = self.footer.as_mut() {
                frust::authoring::route_event_single(pod, ctx, event);
            }
            return EventResult::Ignored;
        }
        // 1. **The barrier gate — once, for every arm below.** A barrier with
        //    no weight on screen swallows nothing: it claims no focus, captures
        //    no pointer, eats no key, and hands nothing to content whose hit
        //    rects still sit where the panel *rests* (the slide is a paint-time
        //    transform, so the pods never move with it). Past a completed
        //    dismissal — `Phase::Dismissed` is terminal, and a
        //    standalone-composed sheet simply stays mounted there — and on the
        //    still-transparent first frame of an entrance, the event belongs to
        //    whatever is behind this sheet (`docs/REVIEW_FOCUS.md`'s overlay
        //    rule; see `barrier_is_live`). Gating here rather than per-arm is
        //    what makes that rule universal in fact and not just in the module
        //    docs: an ungated key arm keeps eating `Escape` (keys are
        //    focus-routed, so they arrive with no hit test to fail), and an
        //    ungated fallthrough keeps eating wheel/move events at the panel's
        //    resting coordinates — both starving the page below.
        //
        //    **The broadcast arm above is the one deliberate exception**, and
        //    it is not a swallow: a broadcast is not user input, is never
        //    consumed, and must keep reaching content for as long as the sheet
        //    is mounted — a callback deferred inside a dismissed-but-mounted
        //    sheet would otherwise never flush. Nothing else outlives the fade.
        //
        //    An in-flight press is *dropped* here, not swallowed: an invisible
        //    barrier holds no gesture, so the rest of that pointer stream falls
        //    through to the page below.
        if !self.barrier_is_live() {
            self.close_pressed = false;
            self.close_captured = false;
            self.scrim_captured = false;
            return EventResult::Ignored;
        }
        // 2. An in-flight close-affordance press owns the pointer stream.
        if self.close_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            return match p.phase {
                PointerPhase::Move => {
                    self.close_pressed = self.close_rect.contains(p.position);
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    // Fire on up-inside, like every other press in the catalog.
                    if self.dismissable && self.close_rect.contains(p.position) {
                        self.begin_exit();
                    }
                    self.close_pressed = false;
                    self.close_captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    // Cancel never dismisses: the platform took the gesture,
                    // the user did not release it.
                    self.close_pressed = false;
                    self.close_captured = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Down => EventResult::Handled,
            };
        }
        // 3. An in-flight scrim/panel-background press owns the stream. A
        //    dismissal that completes while the finger is still down never
        //    reaches here at all — the gate above drops the capture first.
        if self.scrim_captured {
            let InputEvent::Pointer(p) = event else {
                return EventResult::Handled;
            };
            return match p.phase {
                PointerPhase::Up => {
                    let released_outside = !self.panel.contains(p.position);
                    if self.dismissable && self.scrim_down_outside && released_outside {
                        self.begin_exit();
                    }
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.scrim_captured = false;
                    EventResult::Handled
                }
                _ => EventResult::Handled,
            };
        }
        // 4. Fresh events.
        let InputEvent::Pointer(p) = event else {
            // Escape (when the sheet itself, not deeper content, holds the
            // focus path) begins the exit — content gets first crack at it if
            // it holds the deeper focus path.
            if let InputEvent::Key(key_event) = event
                && self.dismissable
                && key_event.key == Key::Named(NamedKey::Escape)
                && !self.content_is_focused()
            {
                self.begin_exit();
                return EventResult::Handled;
            }
            return self.route_to_content(ctx, event);
        };
        match p.phase {
            PointerPhase::Down => {
                // Claim focus on every Down anywhere in the sheet. Load-
                // bearing: the root treats a Down that bubbles no claim as a
                // blur (`release_focus_session` drops focus + IME state), so
                // the re-claim is what keeps the session alive while this
                // sheet is up. Re-claiming while already focused is a
                // change-guarded no-op — do not add a claim-once guard, it
                // kills the session on the second tap.
                ctx.request_focus();
                // The close affordance arms only from `Shown`: during
                // Enter/Exit the phase machine owns the panel, so the press
                // falls through to the barrier arm below, which swallows it
                // (a press *inside* the panel dismisses nothing).
                // Primary-only: a secondary press arms no affordance.
                if presses(p) && self.phase == Phase::Shown && self.close_rect.contains(p.position)
                {
                    self.close_pressed = true;
                    self.close_captured = true;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.route_to_content(ctx, event) == EventResult::Handled {
                    return EventResult::Handled;
                }
                // The barrier swallows a secondary press like any other, but
                // arms no scrim dismiss from it.
                if !presses(p) {
                    return EventResult::Handled;
                }
                self.scrim_captured = true;
                self.scrim_down_outside = !self.panel.contains(p.position);
                ctx.capture_pointer();
                EventResult::Handled
            }
            _ => self.route_to_content(ctx, event),
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.title_text.clone();
        let dismissable = self.dismissable;
        ctx.push_container(
            Role::Dialog,
            |node| {
                node.set_modal();
                node.set_label(label.as_str());
            },
            |ctx| {
                self.title.semantics_child(ctx);
                // The close affordance is this widget's own chrome, so it owns
                // the node for it (mirrors `crate::appbar`'s selection close).
                if dismissable {
                    ctx.push_node(Role::Button, |node| node.set_label("Close"));
                }
                self.body.semantics_child(ctx);
                if let Some(pod) = &self.footer {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(title, body, footer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::NavigatorView;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BuildCtx, KeyEvent, Modifiers, PointerButton, PointerEvent, ScrollDelta, WindowEdgeInsets,
        WindowInsets, any as core_any,
    };
    use frust_core::RenderRoot;
    use frust_widgets::navigator;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

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

    fn build(view: &GlyphSideSheetView<()>) -> GlyphSideSheetWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    /// Records fills, rounded fills, stroked paths, clips, and transform
    /// push/pops — enough to assert the scrim-fade/panel-slide split and the
    /// panel's own chrome independently.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        stroke_paths: Vec<Color>,
        clips: Vec<(Point, Size)>,
        clip_pops: u32,
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
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.stroke_paths.push(*c);
            }
        }
        fn push_clip(&mut self, origin: Point, size: Size) {
            self.clips.push((origin, size));
        }
        fn pop_clip(&mut self) {
            self.clip_pops += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn area() -> Size {
        Size::new(400.0, 600.0)
    }

    /// Frame time by which the 220ms enter has settled to `Phase::Shown`.
    const ENTER_SETTLED_MS: f64 = 400.0;

    fn laid_out(view: &GlyphSideSheetView<()>, size: Size) -> GlyphSideSheetWidget {
        let mut w = build(view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(size));
        w
    }

    /// A laid-out sheet with its enter played out — the resting state every
    /// interaction test starts from (the settled-view constructor).
    fn shown(view: &GlyphSideSheetView<()>) -> GlyphSideSheetWidget {
        let (enter, exit) = resolve_timings(None);
        let mut w = laid_out(view, area());
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(ENTER_SETTLED_MS), enter, exit);
        assert_eq!(w.phase, Phase::Shown, "the sheet settled shown");
        w
    }

    /// Rebuild `w` (built from `prev`) against `next` and lay it out again —
    /// the post-push diff a live app performs on every rebuild, and the vector
    /// the push-time `dismissable` contract is about.
    fn rebuild_into(
        w: &mut GlyphSideSheetWidget,
        prev: &GlyphSideSheetView<()>,
        next: &GlyphSideSheetView<()>,
    ) {
        let mut counter = 0u64;
        View::<()>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(area()));
    }

    /// Paint one unthemed frame and report whether it asked for another.
    fn paint_needs_frame(w: &mut GlyphSideSheetWidget, ms: f64) -> bool {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area(), ft_ms(ms));
        w.paint(&mut pctx, &mut rec);
        pctx.needs_frame()
    }

    /// Play a dismissal out to a settled `Phase::Dismissed` through `paint`
    /// (not bare `advance`), so the frame requests are the ones under test.
    fn dismiss_and_settle(w: &mut GlyphSideSheetWidget) {
        w.begin_exit();
        paint_needs_frame(w, ENTER_SETTLED_MS);
        paint_needs_frame(w, ENTER_SETTLED_MS + 200.0);
        assert_eq!(w.phase, Phase::Dismissed, "the exit ran to completion");
    }

    fn paint_at(w: &mut GlyphSideSheetWidget, ms: f64, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::for_test(Point::ZERO, area(), ft_ms(ms)).with_theme(t),
            None => PaintCtx::for_test(Point::ZERO, area(), ft_ms(ms)),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn dispatch(w: &mut GlyphSideSheetWidget, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = &mut ();
        let mut ctx = EventCtx::new(state_any, Point::ZERO, area());
        w.event(&mut ctx, event)
    }

    fn sheet() -> GlyphSideSheetView<()> {
        glyph_side_sheet("Filters", leaf_any(300.0, 200.0))
    }

    /// A body that fills its slot and consumes **every** event routed to it —
    /// the discriminator a `leaf_any` body cannot give, since a leaf ignores
    /// everything and so cannot tell "the sheet routed nothing" apart from
    /// "the content declined it".
    struct Greedy;
    /// Retained widget for [`Greedy`].
    struct GreedyWidget;

    impl View<()> for Greedy {
        type Element = GreedyWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> GreedyWidget {
            GreedyWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut GreedyWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for GreedyWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            EventResult::Handled
        }
    }

    /// A sheet whose body swallows anything handed to it.
    fn greedy_sheet() -> GlyphSideSheetView<()> {
        glyph_side_sheet("Filters", Greedy)
    }

    fn scroll_at(x: f64, y: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(x, y),
            delta: ScrollDelta::Lines(0.0, -1.0),
        }
    }

    /// The panel's painted translation along x, out of a recording.
    fn panel_translate(rec: &Recorder) -> f64 {
        let mut pure: Vec<[f64; 6]> = rec
            .transforms
            .iter()
            .map(|t| t.as_coeffs())
            .filter(|c| (c[0], c[3]) == (1.0, 1.0))
            .collect();
        assert_eq!(pure.len(), 1, "one pure translate — the panel's own slide");
        let c = pure.pop().unwrap();
        assert_eq!(c[5], 0.0, "the side sheet slides on x only");
        c[4]
    }

    // -- Settled layout ------------------------------------------------------

    #[test]
    fn layout_pins_a_capped_panel_to_the_trailing_edge_at_full_height() {
        let mut w = laid_out(&sheet(), area());
        // 0.83 * 400 = 332, past the 284 cap.
        assert_eq!(w.panel.width(), GLYPH_SIDE_SHEET_MAX_WIDTH);
        assert_eq!(w.panel.x1, area().width, "flush with the trailing edge");
        assert_eq!(w.panel.y0, 0.0);
        assert_eq!(w.panel.y1, area().height, "full height");

        // On a viewport narrower than the cap's own break-even, the fraction
        // binds instead.
        let narrow = Size::new(300.0, 600.0);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::tight(narrow));
        assert_eq!(size, narrow, "the sheet fills the whole area (scrim)");
        assert!((w.panel.width() - 0.83 * 300.0).abs() < 1e-9);
        assert_eq!(w.panel.x1, narrow.width);
    }

    #[test]
    fn a_footerless_sheet_gives_the_body_everything_below_the_fixed_header() {
        let w = laid_out(&sheet(), area());
        // Zero insets: header total height collapses to the bare
        // HEADER_HEIGHT row, and (per the acceptance contract) the only
        // change from the pre-inset geometry is the new horizontal gutter.
        assert_eq!(
            w.body.origin(),
            Point::new(w.panel.x0 + BODY_PAD_X, HEADER_HEIGHT)
        );
        assert_eq!(
            w.body.size(),
            Size::new(w.panel.width() - 2.0 * BODY_PAD_X, 600.0 - HEADER_HEIGHT)
        );
        assert_eq!(w.body_rect.y1, area().height, "…all the way to the bottom");
        assert_eq!(w.close_rect.height(), CLOSE_TOUCH_TARGET);
        assert!(
            w.close_rect.y0 >= 0.0 && w.close_rect.y1 <= HEADER_HEIGHT,
            "the inflated close target stays inside the header row"
        );
    }

    #[test]
    fn a_footer_pins_to_the_bottom_and_takes_its_height_off_the_body() {
        let view = sheet().footer(leaf_any(120.0, 40.0));
        let w = laid_out(&view, area());
        let footer = w.footer.as_ref().expect("the footer pod was built");

        // Zero bottom inset: numerically identical to the pre-inset height
        // (FOOTER_PAD top + content + FOOTER_PAD bottom); only the footer's
        // *horizontal* inset moved from FOOTER_PAD to the shared BODY_PAD_X
        // gutter.
        let footer_h = HAIRLINE + 2.0 * FOOTER_PAD + 40.0;
        assert_eq!(w.footer_divider_y, area().height - footer_h);
        assert_eq!(
            footer.origin(),
            Point::new(
                w.panel.x0 + BODY_PAD_X,
                w.footer_divider_y + HAIRLINE + FOOTER_PAD
            ),
            "the footer sits inside its own insets, below its hairline"
        );
        assert_eq!(
            w.body.size(),
            Size::new(
                w.panel.width() - 2.0 * BODY_PAD_X,
                600.0 - HEADER_HEIGHT - footer_h
            ),
            "the body is tight-filled between the header and the footer"
        );
        assert_eq!(w.body_rect.y1, w.footer_divider_y);
    }

    #[test]
    fn a_non_dismissable_sheet_reclaims_the_close_reserve_for_its_title() {
        let dismissable = laid_out(&sheet(), area());
        let barrier = laid_out(&sheet().dismissable(false), area());
        assert_eq!(barrier.close_rect, Rect::ZERO, "no affordance to hit");
        assert!(
            barrier.title.size().width >= dismissable.title.size().width,
            "the title is given at least the space the chip used to reserve"
        );
    }

    // -- Safe-area insets (device-gate G1) ------------------------------------
    //
    // `LayoutCtx::set_window_insets` is crate-private to `frust-core`, so —
    // exactly like `crate::appbar`'s own top-inset test — a synthetic inset is
    // injected the one public way: `RenderRoot::set_insets`, with the sheet as
    // the tree's own root view (mirrors `semantics_is_a_modal_dialog_labelled_by_the_title`'s
    // harness below).

    #[test]
    fn layout_consumes_the_top_and_bottom_insets_and_keeps_the_horizontal_gutter_footerless() {
        fn logic(_s: &mut ()) -> GlyphSideSheetView<()> {
            sheet()
        }
        let mut root: RenderRoot<(), GlyphSideSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 24.0, 0.0, 34.0),
            WindowEdgeInsets::ZERO,
        ));
        let mut tcx = TextContext::new();
        root.layout_with_text(area(), &mut tcx as &mut dyn Any);

        let id = root.root_id().expect("root built");
        let w = (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<GlyphSideSheetWidget>()
            .expect("root is a GlyphSideSheetWidget");

        // The panel itself stays full-bleed — its rect is untouched by either
        // inset, painting edge to edge under the status bar.
        assert_eq!(w.panel.y0, 0.0, "the panel paints under the status bar");
        assert_eq!(w.panel.y1, area().height);

        // Header *content* sits below the 24px top inset.
        assert_eq!(w.top_inset, 24.0);
        assert_eq!(w.header_height, HEADER_HEIGHT + 24.0);
        assert!(w.title.origin().y >= 24.0, "title below the inset");
        assert!(w.close_center.y >= 24.0, "close chip below the inset");

        // Body: starts where the inset-grown header band ends, keeps the
        // horizontal gutter, and (with no footer to absorb it) gains the
        // 34px bottom inset on top of its own extent.
        assert_eq!(w.body.origin().y, HEADER_HEIGHT + 24.0);
        assert_eq!(
            w.body.origin().x,
            w.panel.x0 + BODY_PAD_X,
            "body children inset by the shared horizontal gutter"
        );
        assert_eq!(w.body.size().width, w.panel.width() - 2.0 * BODY_PAD_X);
        assert_eq!(
            w.body.size().height,
            area().height - (HEADER_HEIGHT + 24.0) - 34.0,
            "the footerless body's own bottom edge gained the bottom inset"
        );
    }

    #[test]
    fn a_footers_bottom_padding_grows_by_the_bottom_inset_and_keeps_the_horizontal_gutter() {
        fn logic(_s: &mut ()) -> GlyphSideSheetView<()> {
            sheet().footer(leaf_any(120.0, 40.0))
        }
        let mut root: RenderRoot<(), GlyphSideSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 0.0, 0.0, 34.0),
            WindowEdgeInsets::ZERO,
        ));
        let mut tcx = TextContext::new();
        root.layout_with_text(area(), &mut tcx as &mut dyn Any);

        let id = root.root_id().expect("root built");
        let w = (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<GlyphSideSheetWidget>()
            .expect("root is a GlyphSideSheetWidget");

        // The footer's own bottom padding grew by the 34px bottom inset —
        // FOOTER_PAD (top) + content + (FOOTER_PAD + 34px bottom).
        let footer_h = HAIRLINE + 2.0 * FOOTER_PAD + 40.0 + 34.0;
        assert_eq!(w.footer_divider_y, area().height - footer_h);
        let footer = w.footer.as_ref().expect("the footer pod was built");
        assert_eq!(
            footer.origin().x,
            w.panel.x0 + BODY_PAD_X,
            "footer children inset by the shared horizontal gutter"
        );

        // The body's own height still excludes only the (now taller) footer —
        // it gained nothing of its own, since the footer absorbed the inset.
        assert_eq!(
            w.body.size().height,
            area().height - HEADER_HEIGHT - footer_h
        );
        assert_eq!(w.body.origin().x, w.panel.x0 + BODY_PAD_X);
    }

    // -- Enter/exit staging: scrim fades, panel slides, INDEPENDENTLY --------

    #[test]
    fn enter_staging_slides_the_panel_in_and_fades_the_scrim_independently() {
        let mut w = build(&sheet());
        let (enter, exit) = resolve_timings(None);

        let (offset0, scrim0, animating0) = w.advance(ft_ms(0.0), enter, exit);
        assert!(
            (offset0 - 1.0).abs() < 1e-6,
            "panel starts fully off-screen"
        );
        assert!(scrim0.abs() < 1e-6, "scrim starts transparent");
        assert!(animating0);
        assert_eq!(w.phase, Phase::Enter);

        // Partway through the 220ms enter — short of where the authored
        // back-ease curve overshoots and clamps — the two progress values are
        // independently mid-flight, which is the crux of the requirement.
        let (offset_mid, scrim_mid, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(offset_mid > 0.0 && offset_mid < 1.0);
        assert!(scrim_mid > 0.0 && scrim_mid < 1.0);

        let (offset1, scrim1, animating1) = w.advance(ft_ms(400.0), enter, exit);
        assert!(offset1.abs() < 1e-6, "panel is at rest");
        assert!((scrim1 - 1.0).abs() < 1e-6, "scrim is fully opaque");
        assert!(!animating1);
        assert_eq!(w.phase, Phase::Shown);
    }

    #[test]
    fn exit_is_faster_than_enter_and_reverses_the_staging() {
        let mut w = build(&sheet());
        let (enter, exit) = resolve_timings(None);
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit); // -> Shown
        w.begin_exit();
        assert_eq!(w.phase, Phase::Exit);

        let (offset_seed, scrim_seed, _) = w.advance(ft_ms(400.0), enter, exit);
        assert!(offset_seed.abs() < 1e-6);
        assert!((scrim_seed - 1.0).abs() < 1e-6);

        // Still exiting 149ms in (already done at the 150ms exit, not yet at
        // the 220ms enter — proving exit is the faster driver).
        let (_, _, animating_mid) = w.advance(ft_ms(400.0 + 149.0), enter, exit);
        assert!(animating_mid);

        let (offset_end, scrim_end, animating_end) = w.advance(ft_ms(400.0 + 200.0), enter, exit);
        assert!((offset_end - 1.0).abs() < 1e-6);
        assert!(scrim_end.abs() < 1e-6);
        assert!(!animating_end);
        assert_eq!(w.phase, Phase::Dismissed);
    }

    #[test]
    fn a_dismissal_mid_enter_exits_from_the_partly_entered_offset() {
        let (enter, exit) = resolve_timings(None);
        let mut w = laid_out(&sheet(), area());
        w.advance(ft_ms(0.0), enter, exit);
        let (mid, _, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(mid > 0.0 && mid < 1.0, "partway in, got {mid}");

        w.begin_exit();
        let (seed, _, _) = w.advance(ft_ms(60.0), enter, exit);
        assert!(
            (seed - mid).abs() < 1e-9,
            "the exit resumes from the entering panel's own offset, not from rest"
        );
    }

    #[test]
    fn reduce_motion_collapses_both_ramps_to_one_linear_crossfade() {
        let mut theme = crate::baseline();
        theme.motion.reduce_motion = true;
        let (enter, exit) = resolve_timings(Some(&theme));
        let collapsed = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        assert_eq!(enter, collapsed);
        assert_eq!(exit, collapsed);

        // …and the collapsed ramp really does drive the panel home.
        let mut w = laid_out(&sheet(), area());
        w.advance(ft_ms(0.0), enter, exit);
        let (offset, _, animating) = w.advance(ft_ms(200.0), enter, exit);
        assert!(offset.abs() < 1e-6);
        assert!(!animating);
    }

    #[test]
    fn on_close_fires_once_when_the_exit_completes() {
        let count = Rc::new(Cell::new(0u32));
        let c = count.clone();
        let view = sheet().on_close(move || c.set(c.get() + 1));
        let mut w = build(&view);
        let (enter, exit) = resolve_timings(None);

        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(400.0), enter, exit);
        w.begin_exit();
        w.advance(ft_ms(400.0), enter, exit);
        assert_eq!(count.get(), 0, "on_close waits for the exit to finish");
        w.advance(ft_ms(700.0), enter, exit);
        assert_eq!(count.get(), 1);
        w.advance(ft_ms(900.0), enter, exit);
        assert_eq!(count.get(), 1, "idempotent past Dismissed");
    }

    // -- Paint ---------------------------------------------------------------

    #[test]
    fn paint_emits_the_scrim_before_the_panel_transform_and_slides_on_x_only() {
        let mut w = laid_out(&sheet(), area());
        let rec = paint_at(&mut w, 0.0, None);

        assert_eq!(rec.rects[0].1, area(), "a full-area scrim fill");
        assert!(
            rec.rects[0].2.components[3] < 1e-3,
            "scrim starts transparent"
        );
        assert_eq!(rec.transforms.len(), 1);
        assert_eq!(rec.transform_pops, 1);
        assert!(
            panel_translate(&rec) > 0.0,
            "the panel starts translated out past the trailing edge"
        );
        assert_eq!(rec.clips.len(), 1, "the component clips its body");
        assert_eq!(rec.clip_pops, 1);
    }

    #[test]
    fn unthemed_chrome_paints_the_documented_fallbacks() {
        let view = sheet().footer(leaf_any(120.0, 40.0));
        let mut w = shown(&view);
        let rec = paint_at(&mut w, ENTER_SETTLED_MS, None);

        // Panel chrome, in paint order after the scrim: surface, leading
        // hairline, header divider. (The footer's own hairline comes after the
        // body's fills, so it is matched by position below.)
        assert_eq!(rec.rects[1].2, CONTAINER, "the panel surface");
        assert_eq!(rec.rects[2].2, BORDER, "the leading hairline");
        assert_eq!(rec.rects[2].1.width, BORDER_WIDTH);
        assert_eq!(rec.rects[3].2, DIVIDER, "the header's bottom hairline");
        assert_eq!(rec.rects[3].1.height, HAIRLINE);
        assert!(
            rec.rects.iter().any(|(o, s, c)| {
                o.y == w.footer_divider_y && s.height == HAIRLINE && *c == DIVIDER
            }),
            "the footer's top hairline"
        );

        // The close chip: a circle (radius = half its own diameter) in the
        // raised-overlay fill, with a two-stroke dim-ink glyph over it.
        let (_, size, radius, fill) = rec.rrects[0];
        assert_eq!(size, Size::new(CLOSE_DIAMETER, CLOSE_DIAMETER));
        assert_eq!(radius, CLOSE_DIAMETER / 2.0);
        assert_eq!(fill, CLOSE_FILL);
        assert_eq!(rec.stroke_paths, vec![CLOSE_INK, CLOSE_INK]);
        assert_eq!(rec.clips[0].1, w.body_rect.size());
    }

    #[test]
    fn themed_chrome_resolves_every_value_from_the_theme() {
        let theme = crate::baseline();
        let scheme = theme.scheme();
        let mut w = shown(&sheet());
        let rec = paint_at(&mut w, ENTER_SETTLED_MS, Some(&theme));

        assert_eq!(rec.rects[1].2, scheme.surface_container_high);
        assert_eq!(rec.rects[2].2, scheme.outline);
        assert_eq!(rec.rects[3].2, scheme.outline_variant);
        assert_eq!(rec.rrects[0].3, scheme.surface_container_highest);
        assert_eq!(rec.stroke_paths[0], scheme.on_surface_variant);
    }

    #[test]
    fn a_settled_sheet_requests_no_frames() {
        let mut w = shown(&sheet());
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::for_test(Point::ZERO, area(), ft_ms(ENTER_SETTLED_MS));
        w.paint(&mut pctx, &mut rec);
        assert!(!pctx.needs_frame());
        assert_eq!(panel_translate(&rec), 0.0, "…at rest");
    }

    // -- Dismiss vectors -----------------------------------------------------

    #[test]
    fn a_scrim_tap_outside_the_panel_begins_the_staged_exit() {
        let mut w = shown(&sheet());
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.scrim_captured);
        assert!(w.scrim_down_outside);
        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, Phase::Exit);
    }

    #[test]
    fn a_press_inside_the_panel_is_swallowed_and_dismisses_nothing() {
        let mut w = shown(&sheet());
        let inside = Point::new(w.panel.x0 + 20.0, 300.0);
        assert_eq!(
            dispatch(&mut w, &ev(PointerPhase::Down, inside.x, inside.y)),
            EventResult::Handled,
            "the barrier swallows it — it must not reach the page below"
        );
        dispatch(&mut w, &ev(PointerPhase::Up, inside.x, inside.y));
        assert_eq!(w.phase, Phase::Shown);
    }

    #[test]
    fn the_close_affordance_presses_on_down_and_exits_on_up_inside() {
        let mut w = shown(&sheet());
        let c = w.close_center;

        dispatch(&mut w, &ev(PointerPhase::Down, c.x, c.y));
        assert!(w.close_captured && w.close_pressed);
        // Pressed ink is the accent role while the finger is down.
        let rec = paint_at(&mut w, ENTER_SETTLED_MS, None);
        assert_eq!(rec.stroke_paths, vec![CLOSE_INK_PRESSED, CLOSE_INK_PRESSED]);

        dispatch(&mut w, &ev(PointerPhase::Up, c.x, c.y));
        assert!(!w.close_captured && !w.close_pressed);
        assert_eq!(
            w.phase,
            Phase::Exit,
            "the close affordance takes the same staged exit"
        );
    }

    #[test]
    fn a_close_press_released_outside_the_affordance_dismisses_nothing() {
        let mut w = shown(&sheet());
        let c = w.close_center;
        dispatch(&mut w, &ev(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &ev(PointerPhase::Move, c.x, c.y + 200.0));
        assert!(!w.close_pressed, "the pressed ink drops with the finger");
        dispatch(&mut w, &ev(PointerPhase::Up, c.x, c.y + 200.0));
        assert_eq!(w.phase, Phase::Shown);

        // A cancel is likewise inert.
        dispatch(&mut w, &ev(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &ev(PointerPhase::Cancel, c.x, c.y));
        assert_eq!(w.phase, Phase::Shown);
        assert!(!w.close_captured);
    }

    #[test]
    fn escape_begins_the_staged_exit() {
        let mut w = shown(&sheet());
        dispatch(&mut w, &escape_event());
        assert_eq!(w.phase, Phase::Exit);
    }

    #[test]
    fn a_dismiss_signal_bump_begins_the_staged_exit() {
        let signal = Rc::new(Cell::new(0u64));
        let mut view = sheet();
        view.dismiss_signal = Some(signal.clone());
        let mut w = shown(&view);

        signal.set(1);
        w.observe_dismiss_signal();
        assert_eq!(w.phase, Phase::Exit);
    }

    #[test]
    fn dismissable_false_ignores_the_scrim_and_escape_and_paints_no_affordance() {
        let mut w = shown(&sheet().dismissable(false));
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, Phase::Shown, "scrim tap ignored");
        dispatch(&mut w, &escape_event());
        assert_eq!(w.phase, Phase::Shown, "Escape ignored");

        let rec = paint_at(&mut w, ENTER_SETTLED_MS, None);
        assert!(rec.rrects.is_empty(), "no close chip is painted");
        assert!(rec.stroke_paths.is_empty(), "…and no close glyph either");
    }

    // -- `dismissable` is a push-time contract --------------------------------

    #[test]
    fn flipping_dismissable_off_after_the_push_changes_nothing() {
        let pushed = sheet();
        let mut w = shown(&pushed);
        let close_rect = w.close_rect;
        let title_w = w.title.size().width;

        rebuild_into(&mut w, &pushed, &sheet().dismissable(false));

        // Geometry: the close affordance keeps its hit rect and the title keeps
        // its (narrower) reserve — no relayout was owed, because the flip is
        // read-at-push-only.
        assert_eq!(
            w.close_rect, close_rect,
            "the affordance still has a target"
        );
        assert_eq!(
            w.title.size().width,
            title_w,
            "…and the title its own width"
        );

        // Paint: the chip and its glyph are still there.
        let rec = paint_at(&mut w, ENTER_SETTLED_MS, None);
        assert_eq!(rec.rrects.len(), 1, "the close chip is still painted");
        assert_eq!(rec.stroke_paths, vec![CLOSE_INK, CLOSE_INK]);

        // Behavior: the scrim tap still dismisses, on the pushed value.
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, Phase::Exit, "the scrim tap still dismisses");
    }

    #[test]
    fn flipping_dismissable_on_after_a_non_dismissable_push_changes_nothing() {
        let pushed = sheet().dismissable(false);
        let mut w = shown(&pushed);

        rebuild_into(&mut w, &pushed, &sheet());

        assert_eq!(w.close_rect, Rect::ZERO, "still no affordance to hit");
        let rec = paint_at(&mut w, ENTER_SETTLED_MS, None);
        assert!(rec.rrects.is_empty(), "still no close chip");

        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(w.phase, Phase::Shown, "the scrim tap is still ignored");
        dispatch(&mut w, &escape_event());
        assert_eq!(w.phase, Phase::Shown, "…and so is Escape");
    }

    // -- A faded-out barrier is not a barrier --------------------------------

    #[test]
    fn a_dismissed_sheet_stops_requesting_frames_once_the_barrier_is_gone() {
        // Standalone composition (no `on_close`, so no navigator page pops this
        // sheet out of the tree): it stays mounted at `Phase::Dismissed`.
        let mut w = shown(&sheet());
        w.begin_exit();
        assert!(
            paint_needs_frame(&mut w, ENTER_SETTLED_MS),
            "the exit is running"
        );
        assert!(
            paint_needs_frame(&mut w, ENTER_SETTLED_MS + 200.0),
            "the frame the exit lands on still asks for one more — the rebuild \
             that applies the pop runs there"
        );
        assert_eq!(w.phase, Phase::Dismissed);
        assert!(
            !paint_needs_frame(&mut w, ENTER_SETTLED_MS + 216.0),
            "…and then it goes quiet: `Dismissed` is terminal, so a level test \
             on the phase would spin frames forever"
        );
        assert!(!paint_needs_frame(&mut w, ENTER_SETTLED_MS + 5_000.0));
    }

    #[test]
    fn a_dismissed_sheets_barrier_swallows_no_input() {
        let mut w = shown(&sheet());
        dismiss_and_settle(&mut w);

        // Scrim coordinates: the fill is fully transparent now.
        assert_eq!(
            dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0)),
            EventResult::Ignored,
            "an invisible barrier must let the press reach the page below"
        );
        assert!(!w.scrim_captured, "…and capture nothing");

        // Panel coordinates: the hit rect is unanimated, so the off-screen
        // panel must not swallow there either.
        let inside = Point::new(w.panel.x0 + 20.0, 300.0);
        assert_eq!(
            dispatch(&mut w, &ev(PointerPhase::Down, inside.x, inside.y)),
            EventResult::Ignored,
            "the panel's resting hit rect is off-screen chrome now"
        );
        assert!(!w.scrim_captured);
        assert_eq!(w.phase, Phase::Dismissed, "and nothing re-armed");
    }

    #[test]
    fn a_dismissed_sheet_eats_no_escape() {
        let mut w = shown(&sheet());
        dismiss_and_settle(&mut w);

        // A key is focus-routed, never hit-tested: a standalone sheet that
        // still holds the focus path would go on eating every `Escape` — and
        // answering `Handled` for a `begin_exit` that is a no-op past
        // `Dismissed` — for as long as it stays mounted.
        assert_eq!(
            dispatch(&mut w, &escape_event()),
            EventResult::Ignored,
            "a gone barrier eats no key: `Escape` belongs to the page below"
        );
        assert_eq!(w.phase, Phase::Dismissed, "…and nothing re-armed");
    }

    #[test]
    fn a_dismissed_sheet_routes_no_move_scroll_or_up_to_its_off_screen_content() {
        let mut w = shown(&greedy_sheet());
        let body_origin = w.body.origin();
        let inside = Point::new(body_origin.x + 5.0, body_origin.y + 5.0);

        // While the barrier is live, the body genuinely gets these — the
        // baseline the assertions below are a change from.
        assert_eq!(
            dispatch(&mut w, &ev(PointerPhase::Move, inside.x, inside.y)),
            EventResult::Handled,
            "a live sheet routes to its content"
        );

        dismiss_and_settle(&mut w);

        // The panel's hit rects are unanimated (the slide is a paint-time
        // transform), so a fallthrough left ungated would keep feeding an
        // off-screen body — and starve the page below of its wheel events.
        for event in [
            ev(PointerPhase::Move, inside.x, inside.y),
            ev(PointerPhase::Up, inside.x, inside.y),
            scroll_at(inside.x, inside.y),
        ] {
            assert_eq!(
                dispatch(&mut w, &event),
                EventResult::Ignored,
                "an off-screen panel routes nothing to its content"
            );
        }
        assert!(
            !w.scrim_captured && !w.close_captured,
            "…and captured nothing on the way through"
        );
        assert_eq!(w.phase, Phase::Dismissed, "…and nothing re-armed");
    }

    #[test]
    fn a_barrier_press_is_released_when_the_barrier_fades_out_under_it() {
        let mut w = shown(&sheet());
        dispatch(&mut w, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.scrim_captured, "the barrier took the press");

        // The app closes the sheet while the finger is still down.
        dismiss_and_settle(&mut w);

        assert_eq!(
            dispatch(&mut w, &ev(PointerPhase::Up, 10.0, 10.0)),
            EventResult::Ignored,
            "the captured stream falls through once the barrier is gone"
        );
        assert!(!w.scrim_captured, "…and the capture was dropped");
    }

    #[test]
    fn a_down_reclaims_focus_after_an_external_blur() {
        let view = sheet();
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        // Settle the enter first: a sheet that has not painted yet still sits
        // fully off-screen behind a fully transparent scrim, and a barrier with
        // no weight on screen claims nothing (see `barrier_is_live`).
        let (enter, exit) = resolve_timings(None);
        w.advance(ft_ms(0.0), enter, exit);
        w.advance(ft_ms(ENTER_SETTLED_MS), enter, exit);
        let mut pod = ChildPod::new(Box::new(w));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        pod.layout_child(&mut lctx, &BoxConstraints::tight(area()));

        let mut dummy = ();
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area());
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        }
        assert!(pod.is_focused(), "the first Down claims focus");
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area());
            pod.event_child(&mut ctx, &ev(PointerPhase::Up, 10.0, 10.0));
        }

        // Simulate an external blur (the root's own Down-with-no-claim release
        // path) so the second Down's re-claim is what's under test.
        pod.set_focused(false);
        {
            let s: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(s, Point::ZERO, area());
            pod.event_child(&mut ctx, &ev(PointerPhase::Down, 10.0, 10.0));
        }
        assert!(
            pod.is_focused(),
            "a second Down must re-claim focus after an external blur — this is what \
             keeps the root's focus/IME session alive while the sheet is up"
        );
    }

    // -- Semantics -----------------------------------------------------------

    #[test]
    fn semantics_is_a_modal_dialog_labelled_by_the_title() {
        fn logic(_s: &mut ()) -> GlyphSideSheetView<()> {
            glyph_side_sheet("Filters", leaf_any(300.0, 200.0))
        }
        let mut root: RenderRoot<(), GlyphSideSheetView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area(), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal(), "the sheet node sets the modal flag");
        assert_eq!(node.label(), Some("Filters"));
        assert!(
            !node.children().is_empty(),
            "the header/body subtrees were forwarded"
        );
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Button && n.label() == Some("Close")),
            "the close affordance contributes its own button node"
        );
    }

    // -- Navigator integration ----------------------------------------------

    /// The nav-driven fixture's state: one entry per pop result the navigator
    /// delivered — the pop counter these tests assert on.
    #[derive(Default)]
    struct NavState {
        results: usize,
    }

    /// A fixed-size leaf over `NavState` (the crate fixture's leaf is
    /// `View<()>`-only) — the root page and the sheet's body both.
    struct Fixed {
        size: Size,
    }
    struct FixedWidget {
        size: Size,
    }
    fn fixed(width: f64, height: f64) -> Fixed {
        Fixed {
            size: Size::new(width, height),
        }
    }
    impl View<NavState> for Fixed {
        type Element = FixedWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FixedWidget {
            FixedWidget { size: self.size }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FixedWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for FixedWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// The harness's app logic: the one navigator, rebuilt each pass.
    type NavApp = Box<dyn FnMut(&mut NavState) -> NavigatorView<NavState>>;

    struct Harness {
        root: RenderRoot<NavState, NavigatorView<NavState>>,
        app: NavApp,
        state: NavState,
        tcx: TextContext,
    }

    impl Harness {
        fn new(controller: &NavigatorController<NavState>) -> Self {
            let ctrl = controller.clone();
            let mut harness = Harness {
                root: RenderRoot::new(),
                app: Box::new(move |_: &mut NavState| {
                    navigator(&ctrl, || core_any::<NavState, _>(fixed(400.0, 600.0)))
                }),
                state: NavState::default(),
                tcx: TextContext::new(),
            };
            harness.rebuild();
            harness
        }

        /// Rebuild — applying any queued navigator op, in queue order — and lay
        /// out.
        fn rebuild(&mut self) {
            self.root.rebuild(&mut self.app, &mut self.state);
            self.root
                .layout_with_text(area(), &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self, ms: f64) {
            self.paint_recorded(ms);
        }

        fn paint_recorded(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        /// Mount the pushed sheet and settle its enter animation.
        fn open(&mut self) {
            self.rebuild();
            self.paint(0.0);
            self.paint(ENTER_SETTLED_MS);
        }

        /// Apply whatever the completed exit enqueued, and flush the pop result
        /// (the navigator's own broadcast drain).
        fn flush(&mut self) {
            self.rebuild();
            self.event(&ev(PointerPhase::Move, 5.0, 5.0));
        }
    }

    /// Whether `rec` caught the sheet's scrim mid-fade — the discriminator for
    /// "a staged exit is in flight": a settled scrim sits at exactly
    /// `SCRIM_ALPHA`, and the root page's own full-area fill is opaque.
    fn scrim_is_mid_fade(rec: &Recorder) -> bool {
        rec.rects.iter().any(|(_, size, color)| {
            *size == area() && color.components[3] > 0.0 && color.components[3] < SCRIM_ALPHA
        })
    }

    fn open_sheet(controller: &NavigatorController<NavState>) -> (Harness, GlyphSideSheetHandle) {
        let mut harness = Harness::new(controller);
        let handle = show_glyph_side_sheet(
            controller,
            || glyph_side_sheet("Filters", fixed(300.0, 200.0)),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        harness.open();
        (harness, handle)
    }

    #[test]
    fn show_glyph_side_sheet_pushes_a_transparent_page_with_no_page_transition() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        show_glyph_side_sheet(
            &controller,
            || glyph_side_sheet("Filters", fixed(300.0, 200.0)),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.rebuild();
        let rec = h.paint_recorded(0.0);

        assert_eq!(controller.depth(), 2, "the sheet page is up");
        // `panel_translate` insists there is exactly *one* pure translate in
        // the whole frame — so the page itself is not being transformed by a
        // navigator transition, and the panel's own staging is the only motion
        // (the widget's `TransitionSpec::NONE` contract, end to end).
        assert!(
            panel_translate(&rec) > 0.0,
            "the panel drives its own slide"
        );
        assert!(
            rec.rects
                .iter()
                .any(|(_, s, c)| *s == area() && c.components[3] < 1e-3),
            "…under its own scrim, which starts transparent"
        );
    }

    #[test]
    fn handle_close_plays_the_staged_exit_and_pops_on_completion() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, sheet) = open_sheet(&controller);

        sheet.close();

        // The first paint after the close observes it and seeds the exit; the
        // next is mid-motion, and that motion is the widget's own — a scrim
        // mid-fade under a trailing-bound panel translate.
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        h.rebuild();
        assert_eq!(controller.depth(), 2, "the sheet is exiting, not unmounted");
        assert_eq!(h.state.results, 0, "the pop waits for the exit to finish");
        assert!(scrim_is_mid_fade(&rec), "the scrim is mid-fade");
        assert!(
            panel_translate(&rec) > 0.0,
            "the panel is mid-slide toward the trailing edge"
        );

        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();
        assert_eq!(controller.depth(), 1, "the completed exit popped the page");
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn a_back_press_stages_the_exit_and_leaves_the_depth_alone_until_it_settles() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let (mut h, _sheet) = open_sheet(&controller);

        controller.request_back();
        h.rebuild();
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        h.rebuild();
        assert!(
            scrim_is_mid_fade(&rec),
            "the back press staged the same reverse ramp"
        );
        assert_eq!(controller.depth(), 2, "depth unchanged until it settles");
        assert_eq!(h.state.results, 0);

        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn repeat_closes_and_a_close_mid_exit_still_pop_exactly_once() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let closes = Rc::new(Cell::new(0u32));
        let counter = closes.clone();
        let sheet = show_glyph_side_sheet(
            &controller,
            move || {
                let counter = counter.clone();
                glyph_side_sheet("Filters", fixed(300.0, 200.0))
                    .on_close(move || counter.set(counter.get() + 1))
            },
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        sheet.close();
        sheet.close(); // two bumps before a paint: one observed change
        h.paint(ENTER_SETTLED_MS);
        sheet.close(); // mid-exit: absorbed by `begin_exit`'s phase guard
        h.paint(ENTER_SETTLED_MS + 100.0);
        h.paint(ENTER_SETTLED_MS + 200.0); // exit complete
        sheet.close(); // past `Dismissed`: absorbed too
        h.paint(ENTER_SETTLED_MS + 300.0);
        h.flush();

        assert_eq!(closes.get(), 1, "one close callback — so one pop");
        assert_eq!(h.state.results, 1);
        assert_eq!(controller.depth(), 1);
    }

    #[test]
    fn the_callers_on_close_composes_with_the_pop_and_runs_after_it() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let calls = Rc::new(Cell::new(0u32));
        let counter = calls.clone();
        let push_ctrl = controller.clone();
        let sheet = show_glyph_side_sheet(
            &controller,
            move || {
                let counter = counter.clone();
                let ctrl = push_ctrl.clone();
                // The callback issues a navigator op of its own — the exact
                // shape the composition order exists for.
                glyph_side_sheet("Filters", fixed(300.0, 200.0)).on_close(move || {
                    counter.set(counter.get() + 1);
                    ctrl.push(|| core_any::<NavState, _>(fixed(400.0, 600.0)));
                })
            },
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        sheet.close();
        h.paint(ENTER_SETTLED_MS);
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();

        assert_eq!(calls.get(), 1, "the caller's `on_close` ran exactly once");
        assert_eq!(h.state.results, 1, "…and the wired pop ran too, once");
        assert_eq!(
            controller.depth(),
            2,
            "pop first, callback second: the page the callback pushed survives"
        );
    }

    #[test]
    fn a_non_dismissable_sheet_vetoes_back_but_still_closes_programmatically() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let sheet = show_glyph_side_sheet(
            &controller,
            || glyph_side_sheet("Filters", fixed(300.0, 200.0)).dismissable(false),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        // The barrier holds against the user: `BackPolicy::Veto` never reaches
        // the dismiss signal, so no exit begins.
        controller.request_back();
        h.rebuild();
        h.paint(500.0);
        h.paint(900.0);
        h.flush();
        assert_eq!(controller.depth(), 2, "a back press leaves it up");
        assert_eq!(h.state.results, 0);

        // The app's own close is not the user's gesture — it closes, staged.
        sheet.close();
        h.paint(1000.0);
        h.rebuild();
        assert_eq!(controller.depth(), 2, "the exit plays before the pop");
        h.paint(1200.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1);
    }

    #[test]
    fn a_post_push_dismissable_flip_leaves_the_back_press_on_its_pushed_policy() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let flag = Rc::new(Cell::new(true));
        let pushed = flag.clone();
        show_glyph_side_sheet(
            &controller,
            move || glyph_side_sheet("Filters", fixed(300.0, 200.0)).dismissable(pushed.get()),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        h.open();

        // The app flips the flag off after the push and rebuilds, so every
        // later page diff carries `dismissable(false)`. The pushed
        // `BackPolicy::DismissAnimated` cannot follow it, so the widget must
        // not follow it either.
        flag.set(false);
        h.rebuild();
        let rec = h.paint_recorded(ENTER_SETTLED_MS);
        assert!(
            !rec.rrects.is_empty(),
            "the close affordance survives the flip — it is the pushed value that binds"
        );

        controller.request_back();
        h.rebuild();
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        assert!(
            scrim_is_mid_fade(&rec),
            "the back press still stages the exit, on the policy it was pushed with"
        );
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn a_flip_before_the_pages_first_build_still_binds_the_pushed_value() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let flag = Rc::new(Cell::new(true));
        let pushed = flag.clone();
        show_glyph_side_sheet(
            &controller,
            move || glyph_side_sheet("Filters", fixed(300.0, 200.0)).dismissable(pushed.get()),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );

        // The flip lands in the window between the push and the navigator's
        // *first* rebuild, so the widget's own `build` — not just a later diff
        // — sees the flipped value. It must still be built from the value the
        // pushed `BackPolicy::DismissAnimated` was fixed from.
        flag.set(false);
        h.open();

        let rec = h.paint_recorded(ENTER_SETTLED_MS);
        assert!(
            !rec.rrects.is_empty(),
            "the close affordance is the pushed value's, not the flipped one's"
        );

        // …and the barrier answers to the same value: the scrim tap dismisses.
        h.event(&ev(PointerPhase::Down, 10.0, 10.0));
        h.event(&ev(PointerPhase::Up, 10.0, 10.0));
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        assert!(
            scrim_is_mid_fade(&rec),
            "the scrim tap staged the exit the pushed policy promises"
        );
        h.paint(ENTER_SETTLED_MS + 200.0);
        h.flush();
        assert_eq!(controller.depth(), 1);
        assert_eq!(h.state.results, 1, "exactly one pop");
    }

    #[test]
    fn a_flip_before_a_non_dismissable_pages_first_build_still_bars_the_user() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let flag = Rc::new(Cell::new(false));
        let pushed = flag.clone();
        show_glyph_side_sheet(
            &controller,
            move || glyph_side_sheet("Filters", fixed(300.0, 200.0)).dismissable(pushed.get()),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );

        // The inverse window: this page was pushed `BackPolicy::Veto` with no
        // dismiss signal wired to the navigator, so a widget built from the
        // flipped value would offer a close affordance and a dismissing scrim
        // the host page never agreed to.
        flag.set(true);
        h.open();

        let rec = h.paint_recorded(ENTER_SETTLED_MS);
        assert!(
            rec.rrects.is_empty(),
            "no close affordance: the page was pushed non-dismissable"
        );

        h.event(&ev(PointerPhase::Down, 10.0, 10.0));
        h.event(&ev(PointerPhase::Up, 10.0, 10.0));
        h.paint(ENTER_SETTLED_MS);
        let rec = h.paint_recorded(ENTER_SETTLED_MS + 75.0);
        assert!(
            !scrim_is_mid_fade(&rec),
            "the scrim tap is barred, on the pushed value"
        );
        h.flush();
        assert_eq!(controller.depth(), 2, "the sheet is still up");
        assert_eq!(h.state.results, 0, "and nothing popped");
    }

    #[test]
    fn a_close_before_the_sheet_first_paints_is_still_honoured() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut h = Harness::new(&controller);
        let sheet = show_glyph_side_sheet(
            &controller,
            || glyph_side_sheet("Filters", fixed(300.0, 200.0)),
            |state: &mut NavState, _result: PopResult| state.results += 1,
        );
        // Closed before the queued push has even been applied: the widget must
        // start at generation 0 and observe the bump on its first paint.
        sheet.close();
        h.rebuild();
        h.paint(0.0);
        h.paint(100.0);
        h.paint(300.0);
        h.flush();

        assert_eq!(controller.depth(), 1, "the sheet closed itself out");
        assert_eq!(h.state.results, 1);
    }

    // -- Typeface: the title follows its type-scale role ------------------

    /// A sheet whose body paints no text, so its only glyph run is the title.
    #[cfg(feature = "bundled-fonts")]
    fn filters(_: &mut ()) -> GlyphSideSheetView<()> {
        glyph_side_sheet("Filters", frust::SizedBox::<()>(Some(200.0), Some(100.0)))
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_paints_in_its_role_face_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let faces = painted_faces(filters, crate::baseline(), area());
        assert_eq!(faces, [Face::SpaceMono]);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_title_follows_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let (before, after) = faces_across_a_live_swap(filters, area());
        assert_eq!(before, [Face::SpaceMono]);
        assert_eq!(after, [Face::PlexMono]);
    }
}
