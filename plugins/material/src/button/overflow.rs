// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// whose `lib/components/buttons/` tree is itself vendored from m3e_buttons
// (MIT, © 2026 Mudit Purohit) — `components/m3e_overflow_strategy.dart`
// (the abstract family + its `id`s), `components/m3e_no_overflow_strategy.dart`,
// `components/m3e_scroll_overflow_strategy.dart`, and
// `styles/m3e_overflow_bottom_sheet_decoration.dart` +
// `../toggle_button_group/components/m3e_toggle_button_group_layout.dart`'s
// trigger-opens-a-sheet flow (the bottom-sheet strategy's real upstream
// home); `components/m3e_base_button_state.dart:177` for a single
// `M3EButton`'s own (unconditional) baseline behavior.
// Porting decision: see this file's module doc, "Where these strategies
// really live upstream" — this crate adapts a `ButtonGroup`-level
// abstraction onto a single button's own seam; frust has no `ButtonGroup`
// overflow story yet (`p2-15`'s job).

//! Overflow strategies for [`super::ButtonWidget`]'s label: what happens once
//! a layout pass finds [`super::ContentMetrics::overflows`] true.
//!
//! # Where these strategies really live upstream
//!
//! Read closely, `M3EOverflowStrategy` (`m3e_overflow_strategy.dart`) is
//! *not* a single button's own overflow behavior — it is an
//! `M3EButtonGroup`/`M3EToggleButtonGroup`-level abstraction over a **list**
//! of button actions that does not all fit the group's row (`buildLayout`
//! takes `actions: List<M3EButtonGroupAction>`, `visibleCount`, a
//! `buildOverflowTrigger`, a `showOverflowMenu`, …). A lone `M3EButton`'s
//! label has exactly one upstream behavior, unconditionally: ellipsize
//! (`m3e_base_button_state.dart:177`'s `overflow: TextOverflow.ellipsis`) —
//! there is no `M3EButton.overflowStrategy` prop anywhere in the reference.
//!
//! The group family's three concrete shapes are `M3ENoOverflowStrategy`
//! (render every action inline, no trigger —
//! `m3e_no_overflow_strategy.dart`), `M3EScrollOverflowStrategy` (wrap the
//! row in a `SingleChildScrollView`, no trigger either —
//! `m3e_scroll_overflow_strategy.dart:47`), and the group's default `menu`
//! overflow mode's own trigger, which opens a picker styled by
//! `M3EOverflowBottomSheetDecoration` when the app configures a bottom sheet
//! presentation (`m3e_toggle_button_group_layout.dart`'s
//! `_onCustomOverflowPressed` → `showOverflowMenu` → `onItemSelected` flow,
//! `m3e_overflow_bottom_sheet_decoration.dart` for the sheet's own styling
//! knobs).
//!
//! `frust_material` has no `ButtonGroup`-level overflow story yet (that is a
//! later task's job) but already carries a single-button
//! `ContentMetrics`/[`super::OverflowObserver`] seam built for exactly this
//! question — its own doc anticipated this follow-up. This module adapts the
//! group family's three-way *semantic* (do nothing extra / let the content
//! pan within its own box / defer to a bigger surface) onto that
//! single-button seam, porting each strategy's actual mechanism (ellipsize /
//! user-drag pan / defer-off-widget) faithfully at the granularity frust has
//! a widget for today, rather than inventing a `ButtonGroup` this task does
//! not own.
//!
//! # The three strategies
//!
//! - [`OverflowStrategy::None`] (default) — today's baseline and the only
//!   change from "no strategy installed at all": the label ellipsizes to the
//!   width available to it, exactly matching *every* `M3EButton`'s one real
//!   behavior (`m3e_base_button_state.dart:177`). Named for
//!   `M3ENoOverflowStrategy`'s identical "nothing extra" idea one level up
//!   (render inline, no trigger, `id => 'none'`).
//! - [`OverflowStrategy::Scroll`] — a **user drag** pans the full,
//!   unellipsized label within the button's own content box, clipped at its
//!   edges. Ported from `M3EScrollOverflowStrategy`'s
//!   `SingleChildScrollView(scrollDirection: direction, primary: false,
//!   child: ...)` (`m3e_scroll_overflow_strategy.dart:47`, `id => 'scroll'`)
//!   — a plain `SingleChildScrollView` carries no ticker or
//!   `AnimationController` of its own, so this is drag-driven, never an
//!   auto-advancing marquee (that question the task explicitly asked: the
//!   source answers "drag").
//! - [`OverflowStrategy::BottomSheet`] — a press on an overflowing button
//!   does not fire normally; it instead calls
//!   [`super::OverflowObserver::overflow_pressed`], mirroring the group
//!   family's own trigger: pressing an overflow indicator never fires a
//!   hidden action directly, it opens a picker surface first
//!   (`_onCustomOverflowPressed` → `showOverflowMenu`; only *then* does
//!   `onItemSelected` fire the real action, `m3e_toggle_button_group_layout.dart:181`-`:203`).
//!   An installed [`super::OverflowObserver`] is this seam's hook for
//!   showing the actual sheet: [`mod@crate::sheet`]'s `show_bottom_sheet`
//!   needs a `NavigatorController<State>`, which a paint/event-time button
//!   widget has no way to reach — it is erased of its `State` type by the
//!   time [`super::ButtonWidget`] exists, the same erasure
//!   [`super::ButtonDecoration`] lives behind. An app installs a concrete
//!   `OverflowObserver` that already closed over its own
//!   `NavigatorController<AppState>` at the call site where `AppState` is
//!   still concrete — the same shape [`super::ButtonView`]'s own `on_press`
//!   is threaded through type erasure by. If `BottomSheet` is selected but no
//!   observer is installed, a press still fires normally rather than going
//!   silently dead — a deliberate safety net, not a second code path to the
//!   same place.
//!
//! # Selecting one
//!
//! [`super::ButtonView::overflow`] installs the strategy;
//! [`super::ButtonView::overflow_observer`] installs the
//! [`super::OverflowObserver`] the `BottomSheet` strategy calls into (also
//! still notified of every [`super::ContentMetrics::measured`] pass exactly
//! as before, regardless of strategy — that half of the seam is
//! strategy-independent). Only the *overflowing* case differs by strategy —
//! a button whose label fits behaves identically under all three: no clip,
//! no drag tracking, a normal press.

use super::core::ContentMetrics;

/// Which behavior an overflowing label triggers, once a layout pass finds
/// [`super::ContentMetrics::overflows`] true. See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverflowStrategy {
    /// Ellipsize and do nothing else — every real `M3EButton`'s actual
    /// behavior, and this seam's default.
    #[default]
    None,
    /// While overflowing, a horizontal drag pans the unellipsized label
    /// within the content box, clipped at its edges.
    Scroll,
    /// While overflowing, a press notifies
    /// [`super::OverflowObserver::overflow_pressed`] instead of firing the
    /// normal `on_press` — falling back to a normal press if no observer is
    /// installed.
    BottomSheet,
}

/// How far `metrics`' label can pan before its trailing edge reaches the
/// content box's own edge — `0` when nothing needs to move. Shared by the
/// [`OverflowStrategy::Scroll`] wiring in `core.rs`'s `layout`/`event`/`paint`
/// passes so the three agree on one clamp.
pub(super) fn max_scroll(metrics: ContentMetrics) -> f64 {
    (metrics.painted_label_width - metrics.available_label_width).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_strategy_is_none() {
        assert_eq!(OverflowStrategy::default(), OverflowStrategy::None);
    }

    #[test]
    fn max_scroll_is_zero_when_nothing_overflows() {
        let metrics = ContentMetrics {
            natural_label_width: 40.0,
            available_label_width: 100.0,
            painted_label_width: 40.0,
            content_width: 40.0,
        };
        assert_eq!(max_scroll(metrics), 0.0);
    }

    #[test]
    fn max_scroll_is_the_hidden_extent_when_overflowing() {
        let metrics = ContentMetrics {
            natural_label_width: 160.0,
            available_label_width: 100.0,
            painted_label_width: 160.0,
            content_width: 160.0,
        };
        assert_eq!(max_scroll(metrics), 60.0);
    }
}
