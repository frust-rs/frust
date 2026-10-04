// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/selection/ (`m3e_selection.dart`'s `M3ESelection`,
//   `controllers/m3e_selection_controller.dart`'s `M3ESelectionController`,
//   `components/m3e_selection_app_bar.dart`'s `M3ESelectionAppBar`,
//   `components/m3e_selection_scope.dart`'s `M3ESelectionScope`,
//   `components/m3e_selection_leading.dart`'s `M3ESelectionLeading`,
//   `styles/m3e_selection_theme.dart`'s `M3ESelectionTheme`)
// This rework restates the reference's `ChangeNotifier`-backed controller as
// pure functions over an app-owned `BTreeSet<usize>` (this crate's controlled-
// component convention has no self-mutating retained state to notify from),
// and merges `M3ESelection` + `M3ESelectionAppBar` + `M3ESelectionScope` into
// two independent pieces — [`selection_host`] (the item wrapper) and
// [`selection_app_bar`] (the bar) — rather than one `InheritedWidget`-threaded
// host, since every other component in this crate takes its collaborators as
// explicit props. See the module docs' Not ported section for what stays
// unported: `M3ESelectionLeading`'s 3D flip, the per-leading-tap toggle
// target, and `M3ESelection`'s `Scaffold` wrapping.

//! The M3E **selection** pattern: an index-set selection mode over a list of
//! items, entered by a long-press and exited by clearing the selection, plus
//! the **contextual bar** that swaps in over an idle app bar while it is
//! active.
//!
//! # Controller: pure functions, not an owned controller
//!
//! The reference's `M3ESelectionController` is a `ChangeNotifier` an app
//! mutates in place (`select`/`deselect`/`toggle`/`selectAll`/`clear`) and
//! listens to. This crate's controlled-component convention has no
//! self-mutating retained state to notify from (`docs/CODE_STANDARDS.md`'s
//! Interaction Semantics: "controlled components never self-mutate") — so
//! this module ports the same five mutations as pure functions over an
//! app-owned `BTreeSet<usize>`, each returning the *next* set for the app to
//! adopt into its own signal: [`select`], [`deselect`], [`toggle`],
//! [`select_all`], [`cleared`]. [`is_selection_mode`] mirrors the reference's
//! derived `isSelectionMode` getter exactly (`!selected.is_empty()`) — there
//! is no independent "in selection mode" flag anywhere in this module; mode
//! is *always* recomputed from the live `selected` set, which is what makes
//! the controlled semantics below hold. [`is_selected`] and
//! [`all_selected_for`] port the remaining read-only queries
//! (`isSelected`/`allSelectedFor`) verbatim.
//!
//! # Wrapper: [`selection_host`]
//!
//! [`selection_host`] wraps arbitrary [`frust::authoring::AnyView`] item rows
//! (a `list_item`/`card_list` row, or any other content) as a single
//! interactive target *per row* — the same "the whole row owns capture"
//! convention [`mod@super::card_list`] and [`mod@super::list_item`] establish.
//! **An item passed here should not chain its own `on_press`** (or any other
//! row-level press handling): the host owns Down/Move/Up/Cancel for the whole
//! row and never forwards a pointer event into a captured row's children (a
//! caller wanting a *second*, independently-tappable target inside a row —
//! the reference's tap-the-leading-avatar-to-toggle affordance — is exactly
//! what this port does not carry forward; see Not ported below).
//!
//! Two outcomes resolve a tap, mutually exclusively, exactly like the
//! reference's own `_onTap`/`_onLongPress` pairing (see
//! `example/lib/pages/playground/view/selection_playground.dart`'s
//! `_SelectionDemoHostState`, the reference's own worked wiring of
//! `M3ESelectionController` against a plain list):
//!
//! - **Tap** (`Up` released inside the same row a `Down` armed): while
//!   [`is_selection_mode`] reads `true`, fires [`SelectionHostView`]'s
//!   `on_toggle` with the row's index (`_onTap`'s `selection.toggle(index)`
//!   branch); otherwise fires `on_activate` (`_onTap`'s `else` branch — the
//!   row's ordinary action, e.g. opening it).
//! - **Long-press** (held past [`LONG_PRESS_MS`], 500ms): fires `on_toggle`
//!   with the row's index **only when it is not already selected** — ported
//!   verbatim from `_onLongPress`'s own guard (`if (!isSelected(index))
//!   select(index)`), so a long-press can enter or extend selection mode but
//!   never *exits* it by deselecting the row it landed on. Movement past
//!   [`frust::input::TOUCH_SLOP`] from the initiating `Down` cancels the
//!   long-press candidacy (a drag, not a hold) without cancelling the row's
//!   own tap/toggle resolution at `Up`.
//!
//! `paint` is the only pass carrying a clock in this framework, so the
//! long-press threshold is latched there and fires on the next pointer event
//! that arrives after it (a `Move`, or `Up` as the final fallback) — the same
//! `frust_core::mark_pending_result_flush`-unreachable-from-a-plugin
//! constraint [`mod@super::toggle_button`]'s own Long-press section documents
//! at length; this module's [`LongPressState`] is that same shape, ported
//! fresh here (`docs/PLUGINS_CODE_STANDARDS.md`'s design-system charter: no
//! `frust-core` dependency in production, so `frust_widgets::gesture`'s
//! `on_long_press` primitive — which *does* reach that function — is
//! unreachable from this crate either way).
//!
//! `selection_host` paints no chrome of its own (no selection fill, no
//! pressed/hover overlay): the item view a caller supplies owns every visual
//! — a selected row's checkmark/fill is the caller's own
//! `list_item(...).selected(is_selected(&selected, i))` (or a
//! [`crate::checkbox`] wired as its leading slot), exactly as the reference's
//! own `_item`/`_leading` builder keys every visual off `_selection.isSelected
//! (index)` rather than the selection host painting it centrally.
//!
//! # Contextual bar: [`selection_app_bar`]
//!
//! [`selection_app_bar`] wraps an `idle` app bar (or any other intrinsic-
//! height content) and swaps to a contextual toolbar — a leading close
//! affordance, a selected-count label, caller-supplied trailing actions, and
//! an optional select-all row — whenever [`is_selection_mode`] reads `true`
//! for the `selected` set it was built with. The swap is a plain structural
//! rebuild (the idle branch is torn down, the contextual one built, or vice
//! versa — [`frust::authoring::AnyView`]'s own type-changed `rebuild` already
//! takes this shape for a single child; here it is two whole sibling sets of
//! children reconciled the same way), not an animated cross-fade: the
//! reference's own `AnimatedSize` + `AnimatedSwitcher` (220ms enter / 90ms
//! exit, `Cubic(0.2, 0, 0, 1)` / `Cubic(0.4, 0, 1, 1)`) motion is out of this
//! module's v1 scope — see Not ported below.
//!
//! **Metrics cited against `appbar.rs`.** The contextual toolbar matches the
//! idle app bar's own band so the header reads as one continuous surface
//! across the swap: [`APP_BAR_HEIGHT`]/[`APP_BAR_PAD_X`]/[`APP_BAR_GAP`]
//! mirror `appbar.rs`'s private `HEIGHT`/`PAD_X`/`GAP` exactly (64dp / 4dp /
//! 4dp) — duplicated here since those constants are private to that module,
//! the same citation shape [`mod@super::toggle_button`]'s
//! `ToggleButtonSize::square_radius` uses for a private sibling-module value.
//! This bar is its own surface, not a shared base type over the app bar.
//!
//! **Window insets: the contextual branch insets like the idle bar.** An idle
//! [`crate::app_bar`]/[`crate::search_app_bar`] consumes the top, left and
//! right window insets itself by default (Flutter's `AppBar`). The contextual
//! branch does the same: it grows by `padding().top`, lays its close button,
//! count, actions and select-all row out below it and inside the side
//! insets, and paints its `primary_container` fill edge to edge through the
//! whole band. So the swap keeps one height and one set of slot offsets in
//! every composition — unwrapped (both branches consume the inset) or under
//! a consuming `frust::safe_area(..)` (both see 0). The idle view is an
//! opaque [`AnyView`], so its own `.safe_area(..)` cannot be read from here:
//! this bar mirrors the app bar's default, and a caller that opts its idle
//! bar out with `.safe_area(false)` opts this one out too
//! ([`SelectionAppBarView::safe_area`]). The contextual branch does not apply
//! the app bar's window-control `corner_shift`.
//!
//! # Controlled semantics
//!
//! Neither [`SelectionHostWidget`] nor [`SelectionAppBarWidget`] retains an
//! independent "in selection mode" flag: both recompute [`is_selection_mode`]
//! from the `selected` prop on every `rebuild`/`layout`/`paint`. An app that
//! clears its own `selected` set externally (a system-back handler, a
//! "done" action elsewhere in the UI) collapses the contextual bar back to
//! idle and restores tap-to-activate on the very next rebuild — no
//! `on_clear`/`on_toggle` round trip is needed to leave selection mode, only
//! to *enter* or *extend* it (mirrors the reference's own `PopScope`
//! guidance: "system back clears selection", documented on [`M3ESelection`]
//! → [`selection_host`]/[`selection_app_bar`] here — see this module's own
//! doc example on [`selection_app_bar`]).
//!
//! # Not ported
//!
//! * `M3ESelectionLeading`'s 3D Y-axis flip (`Transform`+`Matrix4`, a
//!   perspective rotation between the unselected and selected leading faces).
//!   This framework's [`frust::authoring::PaintScene`] takes only 2D affine
//!   paint calls — no perspective/3D primitive exists to port a `rotateY`
//!   flip onto. A caller wanting a selected-state leading visual swaps its
//!   own leading `AnyView` (e.g. an avatar for a [`crate::checkbox`]) keyed
//!   off [`is_selected`], the same shape `list_item`'s own `leading` slot
//!   already takes any view.
//! * The reference's per-leading tap target (`M3ESelectionLeading(onTap: () =>
//!   controller.toggle(index))`, a *second* independently-tappable zone
//!   layered over just the leading slot). [`selection_host`] makes the whole
//!   row one interactive target, the single-target-per-row convention every
//!   other interactive row in this catalog follows
//!   ([`mod@super::list_item`], [`mod@super::card_list`]).
//! * `M3ESelection`'s `Scaffold` wrapping (`scaffold`/`backgroundColor`/
//!   `resizeToAvoidBottomInset`) — Flutter-`Scaffold`-specific chrome this
//!   crate has no analogue for; a caller composes [`selection_app_bar`] above
//!   [`selection_host`] in its own layout, the same way an idle `app_bar` and
//!   its body are composed today (no scaffold widget exists in this crate).
//! * `M3ESelectionScope`'s `InheritedWidget` context resolution
//!   (`M3ESelectionAppBar`'s optional-controller-falls-back-to-scope
//!   pattern). Every collaborator here is an explicit prop instead, matching
//!   every other component in this crate.
//! * The `AnimatedSize`/`AnimatedSwitcher` contextual-bar transition — see
//!   the Contextual bar section above.
//!
//! # Attribution
//!
//! See `plugins/material/NOTICE`'s "MIT License — Additional Copyright Holders
//! (Vendored Components)" section and its Module Attribution Header
//! Convention. `MaterialTokens`-only: no new theme-extension type is
//! introduced here (unlike the reference's own `M3ESelectionTheme`) — every
//! color/shape resolution below reads `Theme`/`ColorScheme` directly, the
//! same pattern `list_item`/`card_list` already establish.

use std::collections::BTreeSet;
use std::rc::Rc;

use frust::authoring::text::{FontWeight, LineHeight};
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx,
    ThemeTextColor, ThemeTextType, View, Widget, any,
};
use frust::input::TOUCH_SLOP;
use frust::{FrameTime, Theme, icon, text};
use kurbo::{Point, Size};
use peniko::Color;

use crate::icon_button::icon_button;
use crate::press::presses;

// ---- Metrics cited against `appbar.rs` (private there; see the module docs'
// Contextual bar section) --------------------------------------------------

/// Small/center-aligned app bar container height, in logical px — mirrors
/// `appbar.rs`'s private `HEIGHT`.
const APP_BAR_HEIGHT: f64 = 64.0;
/// Horizontal inset from the bar's leading/trailing edges — mirrors
/// `appbar.rs`'s private `PAD_X`.
const APP_BAR_PAD_X: f64 = 4.0;
/// Gap between adjacent slots — mirrors `appbar.rs`'s private `GAP`
/// (upstream's `theme.appBarTheme.titleGap`).
const APP_BAR_GAP: f64 = 4.0;
/// Leading/trailing action slot width — the reference's own literal
/// (`m3e_selection_app_bar.dart`'s `_actionSlot`, commented there as
/// "M3EIconButton sm target"), carried over as-is rather than resolved from
/// this crate's own `IconButtonSize` table (whose `Sm` hit target is 40dp,
/// not 48 — the reference's own constant, not a token this port re-derives).
const ACTION_SLOT: f64 = 48.0;
/// Height of the select-all row (`M3ESelectionTheme.selectAllHeight`).
const SELECT_ALL_HEIGHT: f64 = 48.0;

/// The count label's M3 `titleLargeEmphasized` type-scale token — mirrors
/// `appbar.rs`'s private `TITLE_SIZE`/`TITLE_LINE_HEIGHT`/`TITLE_WEIGHT`
/// (upstream's `theme.typeScale.titleLarge`).
const COUNT_SIZE: f32 = 22.0;
const COUNT_LINE_HEIGHT: f32 = 28.0;
const COUNT_WEIGHT: FontWeight = FontWeight::MEDIUM;

/// Unthemed-fallback contextual bar container fill (a theme resolves this
/// from `colors.primary_container`).
const CONTEXTUAL_BACKGROUND: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);

/// Accessible label for the contextual bar's close affordance
/// (`M3ESelectionAppBar`'s `tooltip`/`semanticLabel`, both `'Clear
/// selection'`).
const CLEAR_SELECTION_LABEL: &str = "Clear selection";

/// The press duration after which a held row press becomes a long-press
/// (`frust_widgets::gesture`'s and `toggle_button.rs`'s own
/// `LONG_PRESS_MS`). **Community-approximate** — Android's
/// `ViewConfiguration` defaults to ~400-500ms, iOS's
/// `UILongPressGestureRecognizer.minimumPressDuration` to 0.5s.
const LONG_PRESS_MS: f64 = 500.0;

// ---- Controller: pure functions over an app-owned `BTreeSet<usize>` ------

/// Whether `selected` puts the surface in selection mode
/// (`M3ESelectionController.isSelectionMode`).
pub fn is_selection_mode(selected: &BTreeSet<usize>) -> bool {
    !selected.is_empty()
}

/// Whether `index` is selected (`M3ESelectionController.isSelected`).
pub fn is_selected(selected: &BTreeSet<usize>, index: usize) -> bool {
    selected.contains(&index)
}

/// A tristate value for a select-all control given `item_count`
/// (`M3ESelectionController.allSelectedFor`): `Some(true)` when every index
/// is selected, `Some(false)` when none are, `None` when some but not all
/// are.
pub fn all_selected_for(selected: &BTreeSet<usize>, item_count: usize) -> Option<bool> {
    if item_count == 0 || selected.is_empty() {
        return Some(false);
    }
    if selected.len() >= item_count {
        return Some(true);
    }
    None
}

/// The set with `index` selected, if it was not already
/// (`M3ESelectionController.select`).
pub fn select(selected: &BTreeSet<usize>, index: usize) -> BTreeSet<usize> {
    let mut next = selected.clone();
    next.insert(index);
    next
}

/// The set with `index` deselected, if it was selected
/// (`M3ESelectionController.deselect`).
pub fn deselect(selected: &BTreeSet<usize>, index: usize) -> BTreeSet<usize> {
    let mut next = selected.clone();
    next.remove(&index);
    next
}

/// The set with `index`'s membership flipped
/// (`M3ESelectionController.toggle`).
pub fn toggle(selected: &BTreeSet<usize>, index: usize) -> BTreeSet<usize> {
    let mut next = selected.clone();
    if !next.remove(&index) {
        next.insert(index);
    }
    next
}

/// Every index in `0..item_count`, selected
/// (`M3ESelectionController.selectAll`).
pub fn select_all(item_count: usize) -> BTreeSet<usize> {
    (0..item_count).collect()
}

/// The cleared (empty) selection (`M3ESelectionController.clear`).
pub fn cleared() -> BTreeSet<usize> {
    BTreeSet::new()
}

// ---- selection_host: the item wrapper -------------------------------------

type OnIndex<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative selection-mode item wrapper. See the [module docs](self).
pub struct SelectionHostView<State: 'static> {
    items: Vec<AnyView<State>>,
    selected: BTreeSet<usize>,
    enabled: bool,
    on_toggle: OnIndex<State>,
    on_activate: Option<OnIndex<State>>,
}

/// Wrap `items` in the selection gesture router: tap toggles membership while
/// `selected` is non-empty, otherwise fires `on_activate`
/// ([`SelectionHostView::on_activate`]); a long-press selects an unselected
/// row (see the [module docs](self)' Wrapper section). `on_toggle` reports
/// the row index whose membership should flip.
pub fn selection_host<State: 'static, F>(
    items: impl IntoIterator<Item = AnyView<State>>,
    selected: &BTreeSet<usize>,
    on_toggle: F,
) -> SelectionHostView<State>
where
    F: Fn(&mut State, usize) + 'static,
{
    SelectionHostView {
        items: items.into_iter().collect(),
        selected: selected.clone(),
        enabled: true,
        on_toggle: Rc::new(on_toggle),
        on_activate: None,
    }
}

impl<State: 'static> SelectionHostView<State> {
    /// Fire when a row is tapped while `selected` is empty (ordinary row
    /// activation, e.g. opening it). Absent by default: a tap outside
    /// selection mode with no `on_activate` set is simply a no-op.
    pub fn on_activate<F: Fn(&mut State, usize) + 'static>(mut self, on_activate: F) -> Self {
        self.on_activate = Some(Rc::new(on_activate));
        self
    }

    /// Gate the whole router (defaults to `true`): while `false`, no row
    /// captures a press, arms a long-press, or fires either callback.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Latched long-press timing state — mirrors `toggle_button.rs`'s
/// `LongPressState` (see the [module docs](self)' Wrapper section).
#[derive(Debug, Default, Clone, Copy)]
struct LongPressState {
    /// The frame time the capture started, seeded on the first `paint` after
    /// a `Down`.
    press_start: Option<FrameTime>,
    /// Whether the hold has crossed [`LONG_PRESS_MS`] — latched in `paint`,
    /// the only pass with a clock.
    elapsed: bool,
    /// Whether the callback has already fired for this press (fire-exactly-
    /// once).
    fired: bool,
}

impl LongPressState {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The retained widget for a [`SelectionHostView`]. See the [module
/// docs](self).
pub struct SelectionHostWidget {
    items: Vec<ChildPod>,
    selected: BTreeSet<usize>,
    enabled: bool,
    /// Each row's `(y, height)` in local space, recorded during layout — the
    /// hit test in `event` reads this.
    rows: Vec<(f64, f64)>,
    width: f64,
    /// The row index a `Down` armed (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`/loss of enablement.
    armed: Option<usize>,
    /// Where the capturing `Down` landed, in local coordinates — the
    /// long-press slop reference point.
    down_pos: Point,
    long_press: LongPressState,
    on_toggle: ErasedArgCallback<usize>,
    on_activate: Option<ErasedArgCallback<usize>>,
}

impl<State: 'static> View<State> for SelectionHostView<State> {
    type Element = SelectionHostWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SelectionHostWidget {
        let items = self
            .items
            .iter()
            .map(|view| frust::authoring::build_child(view, ctx))
            .collect();
        SelectionHostWidget {
            items,
            selected: self.selected.clone(),
            enabled: self.enabled,
            rows: Vec::new(),
            width: 0.0,
            armed: None,
            down_pos: Point::ZERO,
            long_press: LongPressState::default(),
            on_toggle: frust::authoring::erase_callback_arg(&self.on_toggle),
            on_activate: self
                .on_activate
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectionHostWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_children(
            &prev.items,
            &self.items,
            &mut element.items,
            ctx,
            |view| view,
            |_| None,
        );

        if prev.items.len() != self.items.len() {
            // A structural change invalidates the recorded press: the armed
            // index may now name a different row (or none at all).
            element.armed = None;
            element.long_press.clear();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.selected != self.selected {
            element.selected = self.selected.clone();
            flags |= ChangeFlags::PAINT;
        }

        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                element.armed = None;
                element.long_press.clear();
            }
            flags |= ChangeFlags::PAINT;
        }

        element.on_toggle = frust::authoring::erase_callback_arg(&self.on_toggle);
        element.on_activate = self
            .on_activate
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut SelectionHostWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.items.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl SelectionHostWidget {
    /// The row containing local `pos`, if any — a position in a gap between
    /// two rows (or outside the stack's width) belongs to neither.
    fn row_at(&self, pos: Point) -> Option<usize> {
        if pos.x < 0.0 || pos.x >= self.width {
            return None;
        }
        self.rows
            .iter()
            .position(|(y, height)| pos.y >= *y && pos.y < y + height)
    }

    /// Clear whatever gesture state a press left behind (shared by the
    /// `Up`/`Cancel` arms).
    fn clear_press(&mut self) {
        self.armed = None;
        self.long_press.clear();
    }

    /// Fire a long-press: only when `index` is not already selected — ported
    /// verbatim from the reference's `_onLongPress` guard (see the [module
    /// docs](self)' Wrapper section) — so a long-press never deselects the
    /// row it landed on.
    fn fire_long_press(&mut self, ctx: &mut EventCtx, index: usize) {
        if !self.selected.contains(&index) {
            (self.on_toggle)(ctx, index);
        }
    }

    /// Fire a tap: toggle while in selection mode, activate otherwise (see
    /// the [module docs](self)' Wrapper section).
    fn fire_tap(&mut self, ctx: &mut EventCtx, index: usize) {
        if is_selection_mode(&self.selected) {
            (self.on_toggle)(ctx, index);
        } else if let Some(on_activate) = self.on_activate.as_mut() {
            on_activate(ctx, index);
        }
    }
}

impl Widget for SelectionHostWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.width = width;
        let max_height = bc.max().height;
        let child_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, max_height));

        self.rows.clear();
        let mut y = 0.0;
        for pod in self.items.iter_mut() {
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(0.0, y));
            self.rows.push((y, size.height));
            y += size.height;
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }

        // Long-press threshold latch — `paint` is the only pass with a
        // clock. See the [module docs](self)' Wrapper section.
        if self.enabled && self.armed.is_some() && !self.long_press.elapsed {
            let start = *self.long_press.press_start.get_or_insert(ctx.frame_time());
            let elapsed_ms = ctx.frame_time().saturating_sub(start).as_secs_f64() * 1000.0;
            if elapsed_ms >= LONG_PRESS_MS {
                self.long_press.elapsed = true;
            } else {
                ctx.request_frame();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.items, ctx, event);
        };
        if !self.enabled {
            // A disabled router arms nothing and forwards no pointer event
            // into its rows either — mirrors `card_list`'s disabled
            // early-return.
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.row_at(p.position) else {
                    // The gap between two rows is nobody's target.
                    return EventResult::Ignored;
                };
                self.armed = Some(index);
                self.down_pos = p.position;
                self.long_press.clear();
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(armed) = self.armed else {
                    // No host-level hover chrome to claim — see the [module
                    // docs](self)' Wrapper section (a row's own visuals are
                    // the caller's responsibility).
                    return EventResult::Ignored;
                };
                // Long-press slop check + fire-on-move-arrival (mirrors
                // `toggle_button.rs`'s identical pattern).
                if (p.position - self.down_pos).hypot() > TOUCH_SLOP {
                    self.long_press.clear();
                } else if self.long_press.elapsed && !self.long_press.fired {
                    self.long_press.fired = true;
                    self.fire_long_press(ctx, armed);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed else {
                    return EventResult::Ignored;
                };
                // A held-past-threshold press resolves as a long-press,
                // never also as a tap — mutually exclusive outcomes of the
                // same gesture (the [module docs](self)' Wrapper section).
                if self.long_press.elapsed {
                    if !self.long_press.fired {
                        self.long_press.fired = true;
                        self.fire_long_press(ctx, armed);
                    }
                } else if self.row_at(p.position) == Some(armed) {
                    self.fire_tap(ctx, armed);
                }
                self.clear_press();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.is_none() {
                    return EventResult::Ignored;
                }
                self.clear_press();
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::List,
            |_node| {},
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(items);
}

// ---- selection_app_bar: the contextual bar --------------------------------

type OnClear<State> = Rc<dyn Fn(&mut State)>;
type OnAllSelected<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative selection-aware app bar: `idle` content, swapping to a
/// contextual toolbar while `selected` is non-empty. See the [module
/// docs](self)' Contextual bar section.
pub struct SelectionAppBarView<State: 'static> {
    idle: AnyView<State>,
    selected: BTreeSet<usize>,
    item_count: usize,
    actions: Vec<AnyView<State>>,
    show_select_all: bool,
    select_all_label: String,
    on_clear: OnClear<State>,
    on_all_selected: Option<OnAllSelected<State>>,
    safe_area: bool,
}

/// Create a selection app bar showing `idle` while `selected` is empty, and
/// the contextual toolbar (count + close + actions) otherwise. `on_clear`
/// fires when the close affordance is pressed — the app is the source of
/// truth for `selected`, so this is the *only* way this bar's own close
/// button ever empties it (see the [module docs](self)' Controlled semantics
/// section).
pub fn selection_app_bar<State: 'static, F>(
    idle: AnyView<State>,
    selected: &BTreeSet<usize>,
    item_count: usize,
    on_clear: F,
) -> SelectionAppBarView<State>
where
    F: Fn(&mut State) + 'static,
{
    SelectionAppBarView {
        idle,
        selected: selected.clone(),
        item_count,
        actions: Vec::new(),
        show_select_all: true,
        select_all_label: "Select all".to_string(),
        on_clear: Rc::new(on_clear),
        on_all_selected: None,
        safe_area: true,
    }
}

impl<State: 'static> SelectionAppBarView<State> {
    /// Attach contextual trailing action slots, in reading order
    /// (`M3ESelectionAppBar.actions`).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Whether to show the select-all row in the contextual toolbar.
    /// Defaults to `true`.
    pub fn show_select_all(mut self, show: bool) -> Self {
        self.show_select_all = show;
        self
    }

    /// Override the select-all row's label. Defaults to `"Select all"`.
    pub fn select_all_label(mut self, label: impl Into<String>) -> Self {
        self.select_all_label = label.into();
        self
    }

    /// Fire when the select-all row is toggled — `true` means the app should
    /// adopt [`select_all`]`(item_count)`, `false` means [`cleared`]`()`
    /// (`M3ESelectionAppBar.onAllSelected`). Absent by default: toggling
    /// select-all with no callback set is a no-op.
    pub fn on_all_selected<F: Fn(&mut State, bool) + 'static>(
        mut self,
        on_all_selected: F,
    ) -> Self {
        self.on_all_selected = Some(Rc::new(on_all_selected));
        self
    }

    /// Whether the contextual branch consumes the top, left and right window
    /// insets itself (default `true`), exactly as an idle [`crate::app_bar`]
    /// or [`crate::search_app_bar`] does by default — see the [module
    /// docs](self)' Contextual bar section. The idle view is opaque here, so
    /// this setting cannot be read off it: a caller that passes
    /// `.safe_area(false)` to its idle bar passes it here too, keeping both
    /// branches at the same height and offsets. It never affects the idle
    /// branch, which lays out (and insets) on its own terms.
    pub fn safe_area(mut self, enabled: bool) -> Self {
        self.safe_area = enabled;
        self
    }
}

impl<State: 'static> SelectionAppBarView<State> {
    /// The presence-of-slots shape a rebuild's structural check keys off —
    /// mirrors `list_item.rs`'s `ListItem::shape` pattern.
    fn shape(&self) -> (usize, bool) {
        (self.actions.len(), self.show_select_all)
    }
}

/// Build the close affordance's view (`icon_button` over
/// [`crate::icons::CLOSE`], firing `on_clear` directly) — mirrors
/// `dialog.rs`'s `close_view` helper of the same shape.
fn close_view<State: 'static>(on_clear: OnClear<State>) -> AnyView<State> {
    any(
        icon_button(any(icon(crate::icons::CLOSE)), move |state: &mut State| {
            (on_clear)(state);
        })
        .semantic_label(CLEAR_SELECTION_LABEL),
    )
}

/// Build the selected-count label's view (`titleLargeEmphasized`,
/// `on_primary_container`, family from the theme's `titleLargeEmphasized`
/// role).
fn count_view<State: 'static>(count: usize) -> AnyView<State> {
    any::<State, _>(
        text(count.to_string())
            .size(COUNT_SIZE)
            .weight(COUNT_WEIGHT)
            .line_height(LineHeight::Absolute(COUNT_LINE_HEIGHT))
            .themed_role(ThemeTextColor::OnPrimaryContainer)
            .themed_family(ThemeTextType::TitleLargeEmphasized),
    )
}

/// Build the select-all checkbox's view. Ports `_toggleSelectAll`'s binary
/// flip exactly: the checkbox's own tap-cycle report is ignored (`_next`),
/// and `on_all_selected` instead fires with `!all`, where `all` treats a
/// partial selection the same as none (matching upstream's `all ?? false`).
fn select_all_checkbox_view<State: 'static>(
    selected: BTreeSet<usize>,
    item_count: usize,
    on_all_selected: Option<OnAllSelected<State>>,
) -> AnyView<State> {
    let value = all_selected_for(&selected, item_count);
    any(crate::checkbox::tristate_checkbox::<State, _>(
        value,
        move |state: &mut State, _next: Option<bool>| {
            if let Some(cb) = &on_all_selected {
                cb(state, !matches!(value, Some(true)));
            }
        },
    ))
}

/// Build the select-all row's label view (`bodyLarge` — `text`'s own
/// default size, and the theme's `bodyLarge` family — `on_primary_container`).
fn select_all_label_view<State: 'static>(label: String) -> AnyView<State> {
    any::<State, _>(
        text(label)
            .themed_role(ThemeTextColor::OnPrimaryContainer)
            .themed_family(ThemeTextType::BodyLarge),
    )
}

/// The resolved contextual bar container fill. Themed: `colors.
/// primary_container`. Unthemed: [`CONTEXTUAL_BACKGROUND`] exactly.
fn resolve_contextual_background(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().primary_container,
        None => CONTEXTUAL_BACKGROUND,
    }
}

/// Index map into [`SelectionAppBarWidget::contextual`] — mirrors
/// `list_item.rs`'s `Slots` pattern.
#[derive(Clone, Copy)]
struct ContextualSlots {
    close: usize,
    count_text: usize,
    actions_start: usize,
    actions_len: usize,
    /// `(checkbox index, label index)`, present only while
    /// [`SelectionAppBarView::show_select_all`] is `true`.
    select_all: Option<(usize, usize)>,
}

/// Build the ordered child window `[close, count_text, action.., select_all_
/// checkbox?, select_all_label?]` and the [`ContextualSlots`] index map.
fn build_contextual_children<State: 'static>(
    view: &SelectionAppBarView<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, ContextualSlots) {
    let mut children = Vec::new();

    let close_v = close_view::<State>(view.on_clear.clone());
    children.push(frust::authoring::build_child(&close_v, ctx));
    let close = 0;

    let count_v = count_view::<State>(view.selected.len());
    children.push(frust::authoring::build_child(&count_v, ctx));
    let count_text = 1;

    let actions_start = children.len();
    for action in &view.actions {
        children.push(frust::authoring::build_child(action, ctx));
    }
    let actions_len = view.actions.len();

    let select_all = view.show_select_all.then(|| {
        let cb = select_all_checkbox_view::<State>(
            view.selected.clone(),
            view.item_count,
            view.on_all_selected.clone(),
        );
        let lbl = select_all_label_view::<State>(view.select_all_label.clone());
        let cb_idx = children.len();
        children.push(frust::authoring::build_child(&cb, ctx));
        let lbl_idx = children.len();
        children.push(frust::authoring::build_child(&lbl, ctx));
        (cb_idx, lbl_idx)
    });

    (
        children,
        ContextualSlots {
            close,
            count_text,
            actions_start,
            actions_len,
            select_all,
        },
    )
}

/// Tear down every contextual child pod through the view it was built from,
/// per the [`ContextualSlots`] index map — mirrors `list_item.rs`'s
/// `teardown_children` helper.
fn teardown_contextual_children<State: 'static>(
    view: &SelectionAppBarView<State>,
    slots: ContextualSlots,
    children: &mut [ChildPod],
    ctx: &mut BuildCtx<'_>,
) {
    let close_v = close_view::<State>(view.on_clear.clone());
    frust::authoring::teardown_child(&close_v, &mut children[slots.close], ctx);

    let count_v = count_view::<State>(view.selected.len());
    frust::authoring::teardown_child(&count_v, &mut children[slots.count_text], ctx);

    for i in 0..slots.actions_len {
        frust::authoring::teardown_child(
            &view.actions[i],
            &mut children[slots.actions_start + i],
            ctx,
        );
    }

    if let Some((cb_idx, lbl_idx)) = slots.select_all {
        let cb = select_all_checkbox_view::<State>(
            view.selected.clone(),
            view.item_count,
            view.on_all_selected.clone(),
        );
        frust::authoring::teardown_child(&cb, &mut children[cb_idx], ctx);
        let lbl = select_all_label_view::<State>(view.select_all_label.clone());
        frust::authoring::teardown_child(&lbl, &mut children[lbl_idx], ctx);
    }
}

/// The retained widget for a [`SelectionAppBarView`]. See the [module
/// docs](self).
pub struct SelectionAppBarWidget {
    idle: ChildPod,
    /// `[close, count_text, action.., select_all_checkbox?,
    /// select_all_label?]` — see [`ContextualSlots`].
    contextual: Vec<ChildPod>,
    contextual_slots: ContextualSlots,
    selected: BTreeSet<usize>,
    item_count: usize,
    show_select_all: bool,
    select_all_label: String,
    /// Whether the contextual branch consumes the top/left/right window
    /// insets — see [`SelectionAppBarView::safe_area`].
    safe_area: bool,
}

impl<State: 'static> View<State> for SelectionAppBarView<State> {
    type Element = SelectionAppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SelectionAppBarWidget {
        let (contextual, contextual_slots) = build_contextual_children(self, ctx);
        SelectionAppBarWidget {
            idle: frust::authoring::build_child(&self.idle, ctx),
            contextual,
            contextual_slots,
            selected: self.selected.clone(),
            item_count: self.item_count,
            show_select_all: self.show_select_all,
            select_all_label: self.select_all_label.clone(),
            safe_area: self.safe_area,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectionAppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.idle, &self.idle, &mut element.idle, ctx);

        if prev.shape() != self.shape() {
            teardown_contextual_children(
                prev,
                element.contextual_slots,
                &mut element.contextual,
                ctx,
            );
            let (children, slots) = build_contextual_children(self, ctx);
            element.contextual = children;
            element.contextual_slots = slots;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            let slots = element.contextual_slots;

            let prev_close = close_view::<State>(prev.on_clear.clone());
            let next_close = close_view::<State>(self.on_clear.clone());
            flags |= frust::authoring::rebuild_child(
                &prev_close,
                &next_close,
                &mut element.contextual[slots.close],
                ctx,
            );

            let prev_count = count_view::<State>(prev.selected.len());
            let next_count = count_view::<State>(self.selected.len());
            flags |= frust::authoring::rebuild_child(
                &prev_count,
                &next_count,
                &mut element.contextual[slots.count_text],
                ctx,
            );

            for i in 0..slots.actions_len {
                flags |= frust::authoring::rebuild_child(
                    &prev.actions[i],
                    &self.actions[i],
                    &mut element.contextual[slots.actions_start + i],
                    ctx,
                );
            }

            if let Some((cb_idx, lbl_idx)) = slots.select_all {
                let prev_cb = select_all_checkbox_view::<State>(
                    prev.selected.clone(),
                    prev.item_count,
                    prev.on_all_selected.clone(),
                );
                let next_cb = select_all_checkbox_view::<State>(
                    self.selected.clone(),
                    self.item_count,
                    self.on_all_selected.clone(),
                );
                flags |= frust::authoring::rebuild_child(
                    &prev_cb,
                    &next_cb,
                    &mut element.contextual[cb_idx],
                    ctx,
                );

                let prev_lbl = select_all_label_view::<State>(prev.select_all_label.clone());
                let next_lbl = select_all_label_view::<State>(self.select_all_label.clone());
                flags |= frust::authoring::rebuild_child(
                    &prev_lbl,
                    &next_lbl,
                    &mut element.contextual[lbl_idx],
                    ctx,
                );
            }
        }

        if element.selected != self.selected {
            element.selected = self.selected.clone();
            // The idle/contextual branch itself may have flipped.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.safe_area != self.safe_area {
            element.safe_area = self.safe_area;
            flags |= ChangeFlags::LAYOUT;
        }
        element.item_count = self.item_count;
        element.show_select_all = self.show_select_all;
        element.select_all_label = self.select_all_label.clone();
        flags
    }

    fn teardown(&self, element: &mut SelectionAppBarWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.idle, &mut element.idle, ctx);
        teardown_contextual_children(self, element.contextual_slots, &mut element.contextual, ctx);
    }
}

impl Widget for SelectionAppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };

        if !is_selection_mode(&self.selected) {
            let idle_size = self
                .idle
                .layout_child(ctx, &BoxConstraints::loose(Size::new(width, f64::INFINITY)));
            self.idle.set_origin(Point::ZERO);
            return bc.constrain(Size::new(width, idle_size.height));
        }

        // Consume the same top/left/right window insets an idle top app bar
        // does, so the swap keeps one height and one set of slot offsets in
        // every composition (a consuming parent safe area leaves 0 here, for
        // both branches alike).
        let (pad_l, top, pad_r) = if self.safe_area {
            let padding = ctx.window_insets().padding();
            (
                padding.left.max(0.0),
                padding.top.max(0.0),
                padding.right.max(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };
        let leading_edge = pad_l + APP_BAR_PAD_X;
        let trailing_edge = width - pad_r - APP_BAR_PAD_X;

        let slots = self.contextual_slots;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, APP_BAR_HEIGHT));

        let close_size = self.contextual[slots.close].layout_child(ctx, &slot_bc);
        self.contextual[slots.close].set_origin(Point::new(
            leading_edge + (ACTION_SLOT - close_size.width).max(0.0) / 2.0,
            top + (APP_BAR_HEIGHT - close_size.height) / 2.0,
        ));

        // Trailing actions, right-to-left from the trailing edge — mirrors
        // `appbar.rs`'s own reverse-iteration layout.
        let mut right = trailing_edge;
        for i in (0..slots.actions_len).rev() {
            let idx = slots.actions_start + i;
            let size = self.contextual[idx].layout_child(ctx, &slot_bc);
            right -= size.width;
            self.contextual[idx].set_origin(Point::new(
                right,
                top + (APP_BAR_HEIGHT - size.height) / 2.0,
            ));
            right -= APP_BAR_GAP;
        }

        let count_left = leading_edge + ACTION_SLOT + APP_BAR_GAP;
        let count_max_width = (right - count_left).max(0.0);
        let count_size = self.contextual[slots.count_text].layout_child(
            ctx,
            &BoxConstraints::loose(Size::new(count_max_width, APP_BAR_HEIGHT)),
        );
        self.contextual[slots.count_text].set_origin(Point::new(
            count_left,
            top + (APP_BAR_HEIGHT - count_size.height) / 2.0,
        ));

        let mut height = top + APP_BAR_HEIGHT;
        if let Some((cb_idx, lbl_idx)) = slots.select_all {
            let cb_bc = BoxConstraints::loose(Size::new(f64::INFINITY, SELECT_ALL_HEIGHT));
            let cb_size = self.contextual[cb_idx].layout_child(ctx, &cb_bc);
            self.contextual[cb_idx].set_origin(Point::new(
                leading_edge + (ACTION_SLOT - cb_size.width).max(0.0) / 2.0,
                height + (SELECT_ALL_HEIGHT - cb_size.height) / 2.0,
            ));

            let lbl_left = leading_edge + ACTION_SLOT + APP_BAR_GAP;
            let lbl_max_width = (trailing_edge - lbl_left).max(0.0);
            let lbl_size = self.contextual[lbl_idx].layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(lbl_max_width, SELECT_ALL_HEIGHT)),
            );
            self.contextual[lbl_idx].set_origin(Point::new(
                lbl_left,
                height + (SELECT_ALL_HEIGHT - lbl_size.height) / 2.0,
            ));
            height += SELECT_ALL_HEIGHT;
        }

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !is_selection_mode(&self.selected) {
            self.idle.paint_child(ctx, scene);
            return;
        }
        // Every theme read happens before `ctx` is taken mutably below
        // (`paint_child`) — mirrors `card::CardWidget::paint`'s ordering.
        let theme = Theme::from_paint_ctx(ctx);
        let fill = resolve_contextual_background(theme);
        scene.fill_rect(ctx.origin(), ctx.size(), fill);
        for pod in &mut self.contextual {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if is_selection_mode(&self.selected) {
            frust::authoring::route_event(&mut self.contextual, ctx, event)
        } else {
            frust::authoring::route_event_single(&mut self.idle, ctx, event)
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !is_selection_mode(&self.selected) {
            self.idle.semantics_child(ctx);
            return;
        }
        let count = self.selected.len();
        let label = format!("{count} selected");
        let contextual = &self.contextual;
        ctx.push_container(
            Role::TitleBar,
            |node| {
                node.set_label(label.as_str());
                node.add_action(Action::Click);
            },
            |ctx| {
                for pod in contextual {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(contextual, idle);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use frust::authoring::text::TextContext;
    use kurbo::Vec2;
    use std::any::Any;

    // ---- Controller: pure functions ---------------------------------------

    #[test]
    fn is_selection_mode_matches_reference_getter() {
        assert!(!is_selection_mode(&BTreeSet::new()));
        assert!(is_selection_mode(&BTreeSet::from([2])));
    }

    #[test]
    fn is_selected_reads_membership() {
        let selected = BTreeSet::from([1, 3]);
        assert!(is_selected(&selected, 1));
        assert!(!is_selected(&selected, 2));
    }

    #[test]
    fn all_selected_for_matches_reference_tristate_formula() {
        assert_eq!(all_selected_for(&BTreeSet::new(), 0), Some(false));
        assert_eq!(all_selected_for(&BTreeSet::new(), 3), Some(false));
        assert_eq!(all_selected_for(&BTreeSet::from([0, 1, 2]), 3), Some(true));
        assert_eq!(
            all_selected_for(&BTreeSet::from([0, 1, 2, 5]), 3),
            Some(true)
        );
        assert_eq!(all_selected_for(&BTreeSet::from([0]), 3), None);
    }

    #[test]
    fn select_deselect_toggle_and_select_all_are_pure_next_state_transforms() {
        let empty = BTreeSet::new();
        assert_eq!(select(&empty, 2), BTreeSet::from([2]));
        assert_eq!(empty, BTreeSet::new(), "select must not mutate its input");

        let some = BTreeSet::from([1, 2]);
        assert_eq!(deselect(&some, 1), BTreeSet::from([2]));
        assert_eq!(
            deselect(&some, 9),
            some,
            "deselecting an absent index is a no-op value"
        );

        assert_eq!(toggle(&some, 1), BTreeSet::from([2]), "toggle off");
        assert_eq!(toggle(&some, 5), BTreeSet::from([1, 2, 5]), "toggle on");

        assert_eq!(select_all(3), BTreeSet::from([0, 1, 2]));
        assert_eq!(select_all(0), BTreeSet::new());
        assert_eq!(cleared(), BTreeSet::new());
    }

    // ---- selection_host -----------------------------------------------------

    const ROW_HEIGHT: f64 = 40.0;
    const WIDTH: f64 = 200.0;

    fn build<S: 'static>(view: &SelectionHostView<S>) -> SelectionHostWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SelectionHostWidget, height: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(WIDTH, height)))
    }

    fn rows<S: 'static>(n: usize) -> Vec<AnyView<S>> {
        (0..n)
            .map(|_| any::<S, _>(frust::SizedBox(Some(10.0), Some(ROW_HEIGHT))))
            .collect()
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[derive(Default)]
    struct Log {
        toggled: Vec<usize>,
        activated: Vec<usize>,
    }

    fn dispatch(w: &mut SelectionHostWidget, state: &mut Log, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(WIDTH, ROW_HEIGHT * 3.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn layout_stacks_rows_vertically_with_no_gap() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(3), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size, Size::new(WIDTH, ROW_HEIGHT * 3.0));
        assert_eq!(
            w.rows,
            vec![
                (0.0, ROW_HEIGHT),
                (ROW_HEIGHT, ROW_HEIGHT),
                (ROW_HEIGHT * 2.0, ROW_HEIGHT)
            ]
        );
    }

    #[test]
    fn tap_outside_selection_mode_activates_not_toggles() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.activated, vec![0]);
        assert!(state.toggled.is_empty());
    }

    #[test]
    fn tap_in_selection_mode_toggles_not_activates() {
        let selected = BTreeSet::from([1]);
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();
        // Tap the first (unselected) row: toggle(0), never activate.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.toggled, vec![0]);
        assert!(state.activated.is_empty());
    }

    #[test]
    fn tap_up_outside_the_armed_row_fires_nothing() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 5.0, ROW_HEIGHT + 5.0),
        );
        assert!(state.activated.is_empty());
        assert!(state.toggled.is_empty());
    }

    #[test]
    fn long_press_selects_an_unselected_row_and_suppresses_the_tap() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        // Simulate the threshold already crossed (mirrors
        // `toggle_button.rs`'s identical test shape).
        w.long_press.elapsed = true;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 6.0, 5.0));
        assert_eq!(state.toggled, vec![0], "long-press fired on move-arrival");
        assert!(state.activated.is_empty());

        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 6.0, 5.0));
        assert_eq!(
            state.toggled,
            vec![0],
            "the resolved long-press is not also a tap"
        );
    }

    #[test]
    fn long_press_never_deselects_an_already_selected_row() {
        let selected = BTreeSet::from([0]);
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        w.long_press.elapsed = true;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 6.0, 5.0));
        assert!(
            state.toggled.is_empty(),
            "a long-press on an already-selected row must not fire on_toggle"
        );
    }

    #[test]
    fn long_press_still_fires_at_up_if_no_move_arrives_first() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        w.long_press.elapsed = true;
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.toggled, vec![0]);
    }

    #[test]
    fn movement_past_touch_slop_cancels_long_press_candidacy() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        w.long_press.elapsed = true; // simulate the threshold already crossed
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 5.0 + TOUCH_SLOP + 5.0, 5.0),
        );
        assert!(!w.long_press.elapsed, "a drag cancels the candidacy");
        assert!(state.toggled.is_empty());
    }

    #[test]
    fn cancel_clears_state_without_firing() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.armed.is_some());
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 5.0, 5.0));
        assert!(w.armed.is_none());
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert!(state.toggled.is_empty());
    }

    #[test]
    fn disabled_router_arms_nothing() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i)).enabled(false);
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0)),
            EventResult::Ignored
        );
        assert!(w.armed.is_none());
    }

    #[test]
    fn a_secondary_press_never_arms_a_row() {
        let selected = BTreeSet::new();
        let view: SelectionHostView<Log> =
            selection_host(rows(2), &selected, |s: &mut Log, i| s.toggled.push(i));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let mut state = Log::default();
        let secondary = InputEvent::Pointer(frust::authoring::PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
        assert!(w.armed.is_none());
    }

    #[test]
    fn external_clear_collapses_mode_on_the_next_dispatch() {
        // Controlled semantics: rebuilding with an emptied `selected` flips
        // tap resolution from toggle back to activate with no separate
        // "exit" call (see the module docs' Controlled semantics section).
        let mut counter = 0u64;
        let prev_selected = BTreeSet::from([0]);
        let prev: SelectionHostView<Log> =
            selection_host(rows(2), &prev_selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        let mut w = View::<Log>::build(&prev, &mut BuildCtx::new(&mut counter));
        layout(&mut w, 300.0);

        let next_selected = BTreeSet::new();
        let next: SelectionHostView<Log> =
            selection_host(rows(2), &next_selected, |s: &mut Log, i| s.toggled.push(i))
                .on_activate(|s: &mut Log, i| s.activated.push(i));
        View::<Log>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!is_selection_mode(&w.selected));

        let mut state = Log::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(
            state.activated,
            vec![0],
            "collapsed mode resumes tap-to-activate"
        );
        assert!(state.toggled.is_empty());
    }

    // ---- selection_app_bar --------------------------------------------------

    fn idle_view<S: 'static>() -> AnyView<S> {
        any::<S, _>(frust::SizedBox(Some(0.0), Some(30.0)))
    }

    fn bar_build<S: 'static>(view: &SelectionAppBarView<S>) -> SelectionAppBarWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn bar_layout(w: &mut SelectionAppBarWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    #[test]
    fn idle_mode_uses_the_idle_contents_own_height() {
        let selected = BTreeSet::new();
        let view: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &selected, 3, |_: &mut ()| {});
        let mut w = bar_build(&view);
        let size = bar_layout(&mut w, 300.0);
        assert_eq!(size, Size::new(300.0, 30.0));
    }

    #[test]
    fn contextual_mode_height_matches_app_bar_height_metric() {
        let selected = BTreeSet::from([0]);
        let view: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &selected, 3, |_: &mut ()| {}).show_select_all(false);
        let mut w = bar_build(&view);
        let size = bar_layout(&mut w, 300.0);
        assert_eq!(
            size.height, APP_BAR_HEIGHT,
            "the toolbar band matches appbar.rs's own HEIGHT metric"
        );
    }

    // ---- Window insets: the contextual branch insets like the idle bar ----

    /// A phone in landscape under a status bar: 47px side bands, 24px top.
    fn notched() -> frust::authoring::WindowInsets {
        frust::authoring::WindowInsets::new(
            frust::authoring::WindowEdgeInsets::new(47.0, 24.0, 47.0, 0.0),
            frust::authoring::WindowEdgeInsets::ZERO,
        )
    }

    fn bar_layout_with(
        w: &mut SelectionAppBarWidget,
        width: f64,
        insets: frust::authoring::WindowInsets,
    ) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        lctx.with_window_insets(insets, |ctx| {
            w.layout(ctx, &BoxConstraints::loose(Size::new(width, 400.0)))
        })
    }

    /// A selection bar over a real idle top app bar, with one 48px action on
    /// each branch and no select-all row, so both bands are comparable.
    fn real_bar(selected: &BTreeSet<usize>, safe_area: bool) -> SelectionAppBarView<()> {
        use frust_widgets::test_support::leaf_any;
        let idle = crate::app_bar::<()>("Inbox")
            .leading(leaf_any(48.0, 48.0))
            .actions(vec![leaf_any(48.0, 48.0)])
            .safe_area(safe_area);
        selection_app_bar(any(idle), selected, 3, |_: &mut ()| {})
            .show_select_all(false)
            .actions(vec![leaf_any(48.0, 48.0)])
            .safe_area(safe_area)
    }

    #[test]
    fn idle_to_selection_keeps_height_and_slot_offsets_under_window_insets() {
        let idle_set = BTreeSet::new();
        let selected = BTreeSet::from([0]);

        let mut idle = bar_build(&real_bar(&idle_set, true));
        let idle_size = bar_layout_with(&mut idle, 800.0, notched());
        let mut ctx_w = bar_build(&real_bar(&selected, true));
        let ctx_size = bar_layout_with(&mut ctx_w, 800.0, notched());

        assert_eq!(idle_size, Size::new(800.0, 24.0 + APP_BAR_HEIGHT));
        assert_eq!(ctx_size, idle_size, "the swap keeps the bar's height");

        // The contextual slots sit below the top inset and inside the side
        // ones, at the idle bar's own leading/trailing rule.
        let close = &ctx_w.contextual[ctx_w.contextual_slots.close];
        assert_eq!(
            close.origin().x,
            47.0 + APP_BAR_PAD_X + (ACTION_SLOT - close.size().width).max(0.0) / 2.0
        );
        assert_eq!(
            close.origin().y,
            24.0 + (APP_BAR_HEIGHT - close.size().height) / 2.0
        );
        let action = &ctx_w.contextual[ctx_w.contextual_slots.actions_start];
        assert_eq!(
            action.origin().x + action.size().width,
            800.0 - 47.0 - APP_BAR_PAD_X
        );
        assert_eq!(action.origin().y, 24.0 + (APP_BAR_HEIGHT - 48.0) / 2.0);
        let count = &ctx_w.contextual[ctx_w.contextual_slots.count_text];
        assert_eq!(
            count.origin().x,
            47.0 + APP_BAR_PAD_X + ACTION_SLOT + APP_BAR_GAP
        );

        // The fill covers the whole band, the consumed top inset included.
        let mut scene = RectRecorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, ctx_size);
        ctx_w.paint(&mut pctx, &mut scene);
        assert_eq!(scene.rects[0], (Point::ZERO, ctx_size));
    }

    #[test]
    fn idle_to_selection_stays_continuous_under_a_consuming_safe_area() {
        let idle_set = BTreeSet::new();
        let selected = BTreeSet::from([0]);
        let consumed = notched().consuming(true, true, true, false);

        // Inside a consuming scope both branches see zero padding.
        let mut idle = bar_build(&real_bar(&idle_set, true));
        let idle_size = bar_layout_with(&mut idle, 706.0, consumed);
        let mut ctx_w = bar_build(&real_bar(&selected, true));
        let ctx_size = bar_layout_with(&mut ctx_w, 706.0, consumed);
        assert_eq!(idle_size.height, APP_BAR_HEIGHT);
        assert_eq!(ctx_size, idle_size);
        let close = &ctx_w.contextual[ctx_w.contextual_slots.close];
        assert_eq!(
            close.origin().x,
            APP_BAR_PAD_X + (ACTION_SLOT - close.size().width).max(0.0) / 2.0
        );
        let action = &ctx_w.contextual[ctx_w.contextual_slots.actions_start];
        assert_eq!(
            action.origin().x + action.size().width,
            706.0 - APP_BAR_PAD_X
        );

        // Through a real `frust::safe_area`: one 24px pad, never two.
        let heights: Vec<f64> = [&idle_set, &selected]
            .into_iter()
            .map(|set| {
                let wrapped = frust::safe_area(real_bar(set, true)).bottom(false);
                let mut counter = 0u64;
                let mut w = View::<()>::build(&wrapped, &mut BuildCtx::new(&mut counter));
                let mut tcx = TextContext::new();
                let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
                lctx.with_window_insets(notched(), |ctx| {
                    w.layout(ctx, &BoxConstraints::loose(Size::new(800.0, 400.0)))
                })
                .height
            })
            .collect();
        assert_eq!(heights, vec![24.0 + APP_BAR_HEIGHT; 2]);
    }

    #[test]
    fn safe_area_false_opts_the_contextual_branch_out_like_the_idle_bar() {
        let idle_set = BTreeSet::new();
        let selected = BTreeSet::from([0]);
        let mut idle = bar_build(&real_bar(&idle_set, false));
        let idle_size = bar_layout_with(&mut idle, 800.0, notched());
        let mut ctx_w = bar_build(&real_bar(&selected, false));
        let ctx_size = bar_layout_with(&mut ctx_w, 800.0, notched());
        assert_eq!(idle_size.height, APP_BAR_HEIGHT);
        assert_eq!(ctx_size, idle_size);
        let action = &ctx_w.contextual[ctx_w.contextual_slots.actions_start];
        assert_eq!(
            action.origin().x + action.size().width,
            800.0 - APP_BAR_PAD_X
        );

        // Flipping the setting is a layout change.
        let mut counter = 0u64;
        let prev = real_bar(&selected, false);
        let next = real_bar(&selected, true);
        let flags = View::<()>::rebuild(&next, &prev, &mut ctx_w, &mut BuildCtx::new(&mut counter));
        assert!(flags.needs_layout());
        assert!(ctx_w.safe_area);
    }

    #[test]
    fn contextual_mode_with_select_all_adds_the_select_all_row_height() {
        let selected = BTreeSet::from([0]);
        let view: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &selected, 3, |_: &mut ()| {});
        let mut w = bar_build(&view);
        let size = bar_layout(&mut w, 300.0);
        assert_eq!(size.height, APP_BAR_HEIGHT + SELECT_ALL_HEIGHT);
    }

    #[test]
    fn unthemed_paint_fills_the_contextual_background_only_in_selection_mode() {
        let idle_selected = BTreeSet::new();
        let idle: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &idle_selected, 3, |_: &mut ()| {});
        let mut idle_w = bar_build(&idle);
        bar_layout(&mut idle_w, 300.0);
        let mut idle_scene = RectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 30.0));
        idle_w.paint(&mut ctx, &mut idle_scene);
        assert!(
            idle_scene.rects.is_empty(),
            "idle mode paints no background of its own"
        );

        let contextual_selected = BTreeSet::from([0]);
        let contextual: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &contextual_selected, 3, |_: &mut ()| {});
        let mut contextual_w = bar_build(&contextual);
        let size = bar_layout(&mut contextual_w, 300.0);
        let mut contextual_scene = RectRecorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        contextual_w.paint(&mut ctx, &mut contextual_scene);
        // The contextual background fill is always the *first* rect this
        // widget itself paints (whatever a child icon button/checkbox
        // additionally paints of its own container comes after it).
        assert_eq!(contextual_scene.rects[0].1, size);
        assert!(!contextual_scene.rects.is_empty());
    }

    #[derive(Default)]
    struct RectRecorder {
        rects: Vec<(Point, Size)>,
    }
    impl PaintScene for RectRecorder {
        fn fill_rect(&mut self, o: Point, s: Size, _c: Color) {
            self.rects.push((o, s));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    #[test]
    fn close_button_press_fires_on_clear() {
        #[derive(Default)]
        struct S {
            cleared: u32,
        }
        let selected = BTreeSet::from([0]);
        let view: SelectionAppBarView<S> =
            selection_app_bar(idle_view::<S>(), &selected, 3, |s: &mut S| s.cleared += 1);
        let mut w = bar_build(&view);
        bar_layout(&mut w, 300.0);

        let close_pod = &w.contextual[w.contextual_slots.close];
        let center = close_pod.origin()
            + Vec2::new(close_pod.size().width / 2.0, close_pod.size().height / 2.0);

        let mut state = S::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, APP_BAR_HEIGHT));
        w.event(&mut ctx, &ev(PointerPhase::Down, center.x, center.y));
        w.event(&mut ctx, &ev(PointerPhase::Up, center.x, center.y));
        assert_eq!(state.cleared, 1);
    }

    #[test]
    fn select_all_reports_binary_flip_treating_partial_as_not_all() {
        #[derive(Default)]
        struct S {
            calls: Vec<bool>,
        }

        fn bar(selected: &BTreeSet<usize>) -> SelectionAppBarView<S> {
            selection_app_bar(idle_view::<S>(), selected, 3, |_: &mut S| {})
                .on_all_selected(|s: &mut S, all| s.calls.push(all))
        }

        fn press_checkbox(w: &mut SelectionAppBarWidget, state: &mut S) {
            let (cb_idx, _) = w.contextual_slots.select_all.expect("select-all present");
            let pod = &w.contextual[cb_idx];
            let center = pod.origin() + Vec2::new(pod.size().width / 2.0, pod.size().height / 2.0);
            let state_any: &mut dyn Any = state;
            let mut ctx = EventCtx::new(
                state_any,
                Point::ZERO,
                Size::new(300.0, APP_BAR_HEIGHT + SELECT_ALL_HEIGHT),
            );
            w.event(&mut ctx, &ev(PointerPhase::Down, center.x, center.y));
            w.event(&mut ctx, &ev(PointerPhase::Up, center.x, center.y));
        }

        // None selected -> report `true` (select all).
        let none = BTreeSet::from([0]); // selection mode needs >=1 to show the bar
        let view = bar(&none);
        let mut w = bar_build(&view);
        bar_layout(&mut w, 300.0);
        let mut state = S::default();
        press_checkbox(&mut w, &mut state);
        assert_eq!(
            state.calls,
            vec![true],
            "a partial selection reports true (not-all)"
        );

        // All selected -> report `false` (clear).
        let all = BTreeSet::from([0, 1, 2]);
        let view = bar(&all);
        let mut w = bar_build(&view);
        bar_layout(&mut w, 300.0);
        let mut state = S::default();
        press_checkbox(&mut w, &mut state);
        assert_eq!(state.calls, vec![false]);
    }

    #[test]
    fn external_clear_collapses_the_bar_back_to_idle() {
        // Controlled semantics: no `on_clear` round trip is required to
        // leave selection mode — clearing `selected` externally is enough.
        let mut counter = 0u64;
        let prev_selected = BTreeSet::from([0]);
        let prev: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &prev_selected, 3, |_: &mut ()| {});
        let mut w = View::<()>::build(&prev, &mut BuildCtx::new(&mut counter));
        bar_layout(&mut w, 300.0);
        assert!(is_selection_mode(&w.selected));

        let next_selected = BTreeSet::new();
        let next: SelectionAppBarView<()> =
            selection_app_bar(idle_view(), &next_selected, 3, |_: &mut ()| {});
        View::<()>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(!is_selection_mode(&w.selected));

        let size = bar_layout(&mut w, 300.0);
        assert_eq!(
            size,
            Size::new(300.0, 30.0),
            "layout falls back to idle's own height"
        );
    }

    #[test]
    fn semantics_reports_count_selected_label_in_contextual_mode() {
        fn logic(_s: &mut ()) -> SelectionAppBarView<()> {
            let selected = BTreeSet::from([0, 1]);
            selection_app_bar(idle_view(), &selected, 3, |_: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), SelectionAppBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar node is contributed in selection mode");
        assert_eq!(node.label(), Some("2 selected"));
    }
}
