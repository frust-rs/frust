// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/dropdown_menus/components/` — `m3e_dropdown_menu_panel.dart`
// (the panel body, the loading/error/empty states and the search field) and
// `m3e_dropdown_menu_item.dart` (a row's box, its radius morph and its slots),
// plus `m3e_dropdown_menu_actions.dart`'s `_onItemTap`/`_selectSingleItem`/
// `_canSelectMultiItem` selection rules, retrieved 2026-08-20.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
// Porting decisions: the reference's `_wrapExpandAnimation` scale/opacity ramp
// and its `Positioned.fill` outside-tap `Listener` are **not** ported — the
// merged anchored host owns placement, the ramp and light dismiss (see
// `super`'s header); rows are kept as **data** with cached shaped runs rather
// than as a `ListView` of child widgets, so per-state ink stays expressible
// (the shape `crate::menu::panel` established); and keyboard navigation is an
// addition the reference has no counterpart for.

//! The dropdown's option panel: the width-matched, scrollable list of options
//! plus the search field and the loading/error/empty states that replace it.
//!
//! # Width comes from the trigger, not from the content
//!
//! The reference wraps its follower in `SizedBox(width: renderBox.size.width)`
//! (`m3e_dropdown_menu_panel.dart:35`) — the panel is exactly as wide as the
//! field, whatever the options measure. This port reads the same number from
//! the [`OverlayAnchor`] the field captures into, in its own `layout`. A panel
//! whose anchor has never been painted into (a degenerate `Rect::ZERO` cell)
//! falls back to its natural content width, so a panel mounted before its
//! field has painted is merely mis-sized for one frame rather than invisible.
//!
//! # What scrolls
//!
//! The search field is a **header**, outside the scrollable region — the
//! reference's `Column` puts it above the `Flexible(ListView)`
//! (`m3e_dropdown_menu_panel.dart:107`), so typing never scrolls out of reach.
//! Only the option list scrolls, capped by
//! [`DropdownPanelView::max_height`](super::DropdownView::max_height).
//!
//! # Selection is requested, never applied
//!
//! Every activation reports through `on_change` and the panel's own
//! [`selected`](DropdownPanelView::selected) is left alone until the app feeds
//! one back. The three rules are the reference's, kept exactly:
//!
//! * **single-select toggles** — `toggleOnly` flips the tapped item and clears
//!   every other, so tapping the already-selected option **clears** the
//!   selection rather than re-picking it, and the dropdown closes either way
//!   (`_selectSingleItem`).
//! * **multi-select toggles, capped on the way in only** — `_canSelectMultiItem`
//!   lets a deselect through unconditionally and refuses a *new* selection once
//!   `maxSelections` is reached.
//! * **a disabled row reports nothing at all**, not even a close.
//!
//! # Keyboard
//!
//! Up/Down move the highlight over the selectable rows, Enter activates it, and
//! Escape is deliberately left [`EventResult::Ignored`] so
//! [`crate::overlay::anchored`] dismisses the whole overlay with it. The
//! reference has no keyboard handling to port here; the gap this shares with
//! every other overlay in the catalog is that the panel only receives key
//! events once it holds focus, which it claims on a press inside itself and
//! outside the search field (see [`super`]'s keyboard note).

use frust::Theme;
use frust::authoring::text::TextStyle;
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, CornerRadii,
    CursorIcon, ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, ScrollDelta, SemanticsCtx,
    Size, TypedArgCallback, View, Widget, any, build_child, erase_callback_arg, rebuild_children,
    route_event_single, teardown_child, visit_children,
};
use frust::input::WHEEL_LINE_PX;

use super::{
    DROPDOWN_CONTENT_PADDING, DROPDOWN_EMPTY_TEXT, DROPDOWN_ICON_GAP, DROPDOWN_ICON_SIZE,
    DROPDOWN_ITEM_GAP, DROPDOWN_ITEM_H_PADDING, DROPDOWN_ITEM_HOVER_RADIUS,
    DROPDOWN_ITEM_INNER_RADIUS, DROPDOWN_ITEM_OUTER_RADIUS, DROPDOWN_ITEM_PRESSED_RADIUS,
    DROPDOWN_ITEM_V_PADDING, DROPDOWN_LOADING_PADDING, DROPDOWN_MESSAGE_PADDING,
    DROPDOWN_PANEL_MAX_HEIGHT, DROPDOWN_SEARCH_HINT, DropdownColors, DropdownItem, Glyph, OnOpen,
    OnQuery, Run, SHAPING_INK, container_radius, dropdown_filter, ordered_selection,
};
use crate::card_list::{card_position, card_radii};
use crate::icons;
use crate::interaction::InteractionState;
use crate::overlay::{OverlayAnchor, OverlayElevation, shadow, with_alpha};
use crate::press::presses;
use crate::progress::{ProgressValue, circular_progress};
use crate::text_field::text_field;

/// Left/right and top/bottom margins around the search field
/// (`M3EDropdownSearchStyle.margin`, `EdgeInsets.fromLTRB(12, 8, 12, 4)`).
pub(super) const SEARCH_MARGIN_H: f64 = 12.0;
/// Top margin of the search field (`margin`'s `top`).
pub(super) const SEARCH_MARGIN_TOP: f64 = 8.0;
/// Bottom margin of the search field (`margin`'s `bottom`).
pub(super) const SEARCH_MARGIN_BOTTOM: f64 = 4.0;
/// The search field's own decorated height — [`crate::text_field`]'s fixed
/// decoration box, which is what a mounted field measures.
pub(super) const SEARCH_FIELD_HEIGHT: f64 = 56.0;

/// One rendered option row: its resolved state, its shaped content, and the
/// vertical band it occupies in the list's own content space.
struct Row {
    /// The option's value — the identity a selection is expressed in.
    value: String,
    enabled: bool,
    /// Resolved from the app-confirmed selection at build time.
    selected: bool,
    label: Run,
    /// The trailing check, present only while selected.
    check: Option<Glyph>,
    /// Top of the row's band, in content space.
    top: f64,
    /// The band's full height (the item box plus [`DROPDOWN_ITEM_GAP`]).
    height: f64,
}

/// What the panel shows where the option list would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    /// The option rows (`_buildPanelBody`'s `ListView.separated`).
    Rows,
    /// The centered busy indicator.
    Loading,
    /// A load failure's message.
    Error,
    /// The no-matches message.
    Empty,
}

/// A declarative dropdown option panel. See [`dropdown_panel`].
pub struct DropdownPanelView<State: 'static> {
    items: Vec<DropdownItem>,
    selected: Vec<String>,
    multi: bool,
    max_selections: usize,
    query: String,
    searchable: bool,
    loading: bool,
    error: Option<String>,
    empty_text: String,
    max_height: f64,
    open: bool,
    anchor: OverlayAnchor,
    on_change: TypedArgCallback<State, Vec<String>>,
    on_open: Option<OnOpen<State>>,
    on_query: Option<OnQuery<State>>,
}

/// Build a dropdown option panel over `items` — the list surface, without any
/// anchoring or dismissal of its own.
///
/// This is what [`dropdown`](super::dropdown) mounts inside
/// [`crate::overlay::anchored`]; an app building an ordinary dropdown wants
/// that instead. `on_change(state, values)` reports the selection an activation
/// requests, normalized into item order.
pub fn dropdown_panel<State: 'static, F: Fn(&mut State, Vec<String>) + 'static>(
    items: Vec<DropdownItem>,
    on_change: F,
) -> DropdownPanelView<State> {
    DropdownPanelView {
        items,
        selected: Vec::new(),
        multi: false,
        max_selections: 0,
        query: String::new(),
        searchable: false,
        loading: false,
        error: None,
        empty_text: DROPDOWN_EMPTY_TEXT.to_string(),
        max_height: DROPDOWN_PANEL_MAX_HEIGHT,
        open: true,
        anchor: OverlayAnchor::new(),
        on_change: std::rc::Rc::new(on_change),
        on_open: None,
        on_query: None,
    }
}

impl<State: 'static> DropdownPanelView<State> {
    /// Point the panel at the trigger's captured rect — what width-matches it
    /// to the field (see the [module docs](self)).
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Hand the panel the app-confirmed selection (option values).
    pub fn selected(mut self, selected: Vec<String>) -> Self {
        self.selected = selected;
        self
    }

    /// Allow more than one option at a time.
    pub fn multi(mut self, multi: bool) -> Self {
        self.multi = multi;
        self
    }

    /// Cap a multi-select at `max` options; `0` is unlimited.
    pub fn max_selections(mut self, max: usize) -> Self {
        self.max_selections = max;
        self
    }

    /// Hand the panel the app-confirmed filter query.
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    /// Show the search field above the option list.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }

    /// Show the busy row instead of the option list.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Show a load failure instead of the option list.
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    /// Replace the empty-state message.
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// Cap the panel's height, past which the option list scrolls.
    pub fn max_height(mut self, max_height: f64) -> Self {
        self.max_height = max_height;
        self
    }

    /// Tell the panel whether its host is open. `false` clears the keyboard
    /// highlight and the press machine on the spot; the default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the open callback — fired with `false` after a single-select pick.
    pub fn on_open<F: Fn(&mut State, bool) + 'static>(mut self, on_open: F) -> Self {
        self.on_open = Some(std::rc::Rc::new(on_open));
        self
    }

    /// Set the query callback: each edit of the search field reports the text
    /// it requests.
    pub fn on_query<F: Fn(&mut State, String) + 'static>(mut self, on_query: F) -> Self {
        self.on_query = Some(std::rc::Rc::new(on_query));
        self
    }

    /// The rows this view's props render as — computed identically by `build`
    /// and `rebuild`, so the row list can never drift from the props.
    fn build_rows(&self) -> Vec<Row> {
        dropdown_filter(&self.items, &self.query)
            .into_iter()
            .map(|item| {
                let selected = self.selected.iter().any(|v| v == item.value());
                Row {
                    value: item.value().to_string(),
                    enabled: !item.is_disabled(),
                    selected,
                    label: Run::new(item.label()),
                    // The reference draws the check only while selected
                    // (`m3e_dropdown_menu_item.dart:157`).
                    check: selected.then(|| Glyph::new(icons::CHECK_ROUNDED, DROPDOWN_ICON_SIZE)),
                    top: 0.0,
                    height: 0.0,
                }
            })
            .collect()
    }

    /// Which body the current props render (`_buildPanelBody`'s own order:
    /// loading, then error, then empty, then the rows).
    fn body(&self, rows: &[Row]) -> Body {
        if self.loading {
            Body::Loading
        } else if self.error.is_some() {
            Body::Error
        } else if rows.is_empty() {
            Body::Empty
        } else {
            Body::Rows
        }
    }

    /// The search field's view, in a zero-or-one list so a `searchable` flip
    /// reconciles as an ordinary child-list length change.
    fn search_views(&self) -> Vec<AnyView<State>> {
        if !self.searchable {
            return Vec::new();
        }
        let on_query = self.on_query.clone();
        vec![any(text_field(
            self.query.clone(),
            move |state: &mut State, text| {
                if let Some(on_query) = &on_query {
                    on_query(state, text);
                }
            },
        )
        .label(DROPDOWN_SEARCH_HINT))]
    }

    /// The busy indicator's view, in a zero-or-one list on the same rule.
    fn loading_views(&self) -> Vec<AnyView<State>> {
        if !self.loading {
            return Vec::new();
        }
        vec![any(circular_progress(ProgressValue::Indeterminate))]
    }

    /// The message a non-row body paints: the load failure when there is one,
    /// the empty-state line otherwise.
    fn message(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| self.empty_text.clone())
    }
}

/// The retained widget for a [`DropdownPanelView`].
pub struct DropdownPanelWidget {
    /// Every option, filtered or not — the order a reported selection is
    /// normalized into.
    items: Vec<DropdownItem>,
    rows: Vec<Row>,
    selected: Vec<String>,
    multi: bool,
    max_selections: usize,
    body: Body,
    /// The empty/error line, shaped lazily like every other run.
    message: Run,
    /// Whether [`Self::message`] is an error rather than the empty state.
    message_is_error: bool,
    /// The search field's pod, zero or one.
    search: Vec<ChildPod>,
    /// The busy indicator's pod, zero or one.
    busy: Vec<ChildPod>,
    max_height: f64,
    open: bool,
    anchor: OverlayAnchor,
    on_change: ErasedArgCallback<Vec<String>>,
    on_open: Option<ErasedArgCallback<bool>>,
    /// The latched hovered row (the hover-flag half of the catalog's three-part
    /// hover seam; `paint` self-corrects it from `PaintCtx::is_hovered`).
    hovered: Option<usize>,
    /// The keyboard highlight — independent of `hovered`, since a pointer and
    /// the arrow keys can disagree.
    highlighted: Option<usize>,
    /// The row a live press is armed on.
    pressed: Option<usize>,
    /// Whether that press is currently *inside* its own row.
    press_inside: bool,
    captured: bool,
    scroll: f64,
    /// Top of the scrollable list, in the panel's own space.
    body_top: f64,
    /// The scrollable list's own height.
    viewport: f64,
    content_height: f64,
    width: f64,
    height: f64,
}

impl DropdownPanelWidget {
    /// The panel's resolved width, in logical px.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// The panel's resolved height, in logical px.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// The panel's scroll offset, in logical px.
    pub fn scroll(&self) -> f64 {
        self.scroll
    }

    /// The keyboard-highlighted row, if any.
    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// The `(top, height)` band row `row` occupies in the list's content space.
    #[cfg(test)]
    fn band(&self, row: usize) -> (f64, f64) {
        let row = &self.rows[row];
        (row.top, row.height)
    }

    /// Clear every transient interaction record — what a closing panel and a
    /// row-list replacement both do.
    fn reset_interaction(&mut self) {
        self.hovered = None;
        self.highlighted = None;
        self.pressed = None;
        self.press_inside = false;
        self.captured = false;
    }

    /// Keep the scroll offset inside the scrollable range.
    fn clamp_scroll(&mut self) {
        let max = (self.content_height - self.viewport).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// The search field's placed box, in the panel's own space — `None` when
    /// the panel is not [`searchable`](DropdownPanelView::searchable).
    pub fn search_rect(&self) -> Option<Rect> {
        let pod = self.search.first()?;
        Some(Rect::from_origin_size(pod.origin(), pod.size()))
    }

    /// The row under panel-local `pos`, if any. Only the scrollable list is
    /// hit-tested; the header band above it belongs to the search field.
    fn row_at(&self, pos: Point) -> Option<usize> {
        if self.body != Body::Rows
            || pos.x < 0.0
            || pos.x >= self.width
            || pos.y < self.body_top
            || pos.y >= self.body_top + self.viewport
        {
            return None;
        }
        let y = pos.y - self.body_top + self.scroll;
        self.rows
            .iter()
            .position(|row| y >= row.top && y < row.top + row.height)
    }

    /// Whether row `i` can be activated at all.
    fn activatable(&self, i: usize) -> bool {
        self.rows.get(i).is_some_and(|row| row.enabled)
    }

    /// The selection row `i`'s activation requests, or `None` when the rules
    /// refuse it (`_onItemTap`'s three branches).
    fn requested_selection(&self, i: usize) -> Option<Vec<String>> {
        let row = self.rows.get(i)?;
        if !row.enabled {
            return None;
        }
        let already = self.selected.iter().any(|v| v == &row.value);
        if !self.multi {
            // `toggleOnly`: the tapped item flips, every other clears — so
            // tapping the selected option clears the selection outright.
            return Some(if already {
                Vec::new()
            } else {
                vec![row.value.clone()]
            });
        }
        if already {
            // A deselect is never capped (`_canSelectMultiItem`'s first
            // branch).
            let kept: Vec<String> = self
                .selected
                .iter()
                .filter(|v| *v != &row.value)
                .cloned()
                .collect();
            return Some(ordered_selection(&self.items, &kept));
        }
        if self.max_selections > 0 && self.selected.len() >= self.max_selections {
            return None;
        }
        let mut next = self.selected.clone();
        next.push(row.value.clone());
        Some(ordered_selection(&self.items, &next))
    }

    /// Report row `i`'s activation.
    fn activate(&mut self, ctx: &mut EventCtx, i: usize) {
        let Some(next) = self.requested_selection(i) else {
            return;
        };
        (self.on_change)(ctx, next);
        // The reference closes after a single-select pick and leaves a
        // multi-select open (`_selectSingleItem` vs. `_onItemTap`'s tail).
        if !self.multi
            && let Some(on_open) = self.on_open.as_mut()
        {
            on_open(ctx, false);
        }
    }

    /// Move the highlight `step` rows, skipping disabled ones and stopping at
    /// the ends rather than wrapping.
    fn move_highlight(&mut self, step: isize) -> bool {
        if self.body != Body::Rows || self.rows.is_empty() {
            return false;
        }
        let len = self.rows.len() as isize;
        let mut index = match self.highlighted {
            Some(current) => current as isize,
            // A first Down enters at the top, a first Up at the bottom.
            None => {
                if step > 0 {
                    -1
                } else {
                    len
                }
            }
        };
        loop {
            index += step;
            if index < 0 || index >= len {
                return false;
            }
            if self.rows[index as usize].enabled {
                self.highlighted = Some(index as usize);
                self.reveal(index as usize);
                return true;
            }
        }
    }

    /// Scroll row `i` into the list's viewport.
    fn reveal(&mut self, i: usize) {
        let Some(row) = self.rows.get(i) else {
            return;
        };
        let (top, bottom) = (row.top, row.top + row.height);
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + self.viewport {
            self.scroll = bottom - self.viewport;
        }
        self.clamp_scroll();
    }

    /// The `Widget::event` key arm. Escape is deliberately left for the host.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &Key) -> EventResult {
        match key {
            Key::Named(NamedKey::ArrowDown) => {
                if self.move_highlight(1) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.move_highlight(-1) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::Enter) => {
                let Some(i) = self.highlighted.filter(|i| self.activatable(*i)) else {
                    return EventResult::Ignored;
                };
                self.activate(ctx, i);
                ctx.request_redraw();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    /// The `Widget::event` pointer arm: hover latching, press arming, and
    /// release-to-activate.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        match p.phase {
            PointerPhase::Move if !self.captured => {
                let row = self.row_at(p.position);
                // Claimed from the uncaptured move arm on every qualifying
                // pass, per the catalog's hover seam.
                if row.is_some() {
                    ctx.claim_hover();
                }
                let hovered = row.filter(|i| self.activatable(*i));
                if hovered.is_some() {
                    ctx.set_cursor(CursorIcon::Pointer);
                }
                if self.hovered != hovered {
                    self.hovered = hovered;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Move => {
                // Captured: the pressed wash follows the pointer in and out of
                // the armed row without ending the gesture.
                let inside = self.pressed.is_some() && self.row_at(p.position) == self.pressed;
                if self.press_inside != inside {
                    self.press_inside = inside;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                // Claimed so the arrow keys have somewhere to route; a press
                // that lands in the search field has already been routed there
                // and never reaches this arm.
                ctx.request_focus();
                let Some(row) = self.row_at(p.position).filter(|i| self.activatable(*i)) else {
                    // The panel's own background: left for the host, which
                    // swallows a press inside its content without dismissing.
                    return EventResult::Ignored;
                };
                self.pressed = Some(row);
                self.press_inside = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let released = self.row_at(p.position);
                let armed = self.pressed;
                self.pressed = None;
                self.press_inside = false;
                self.captured = false;
                // Fire on up-inside only, the catalog's press contract.
                if let Some(row) = armed.filter(|row| released == Some(*row)) {
                    self.activate(ctx, row);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            // A `Cancel` arm touches no app state — only the flags the next
            // move re-establishes.
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.press_inside = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    /// The `Widget::event` scroll arm: the same `offset + dy` wheel convention
    /// every scrollable in this catalog shares, clamped to the content range.
    fn handle_scroll(
        &mut self,
        ctx: &mut EventCtx,
        position: Point,
        delta: &ScrollDelta,
    ) -> EventResult {
        if position.y < self.body_top || position.y >= self.body_top + self.viewport {
            return EventResult::Ignored;
        }
        let dy = match delta {
            ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
            ScrollDelta::Pixels(_, y) => *y,
        };
        let before = self.scroll;
        self.scroll += dy;
        self.clamp_scroll();
        if (self.scroll - before).abs() <= f64::EPSILON {
            return EventResult::Ignored;
        }
        ctx.request_redraw();
        EventResult::Handled
    }
}

impl<State: 'static> View<State> for DropdownPanelView<State> {
    type Element = DropdownPanelWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DropdownPanelWidget {
        let rows = self.build_rows();
        let body = self.body(&rows);
        DropdownPanelWidget {
            items: self.items.clone(),
            rows,
            selected: self.selected.clone(),
            multi: self.multi,
            max_selections: self.max_selections,
            body,
            message: Run::new(self.message()),
            message_is_error: self.error.is_some(),
            search: self
                .search_views()
                .iter()
                .map(|view| build_child(view, ctx))
                .collect(),
            busy: self
                .loading_views()
                .iter()
                .map(|view| build_child(view, ctx))
                .collect(),
            max_height: self.max_height,
            open: self.open,
            anchor: self.anchor.clone(),
            on_change: erase_callback_arg(&self.on_change),
            on_open: self.on_open.as_ref().map(erase_callback_arg),
            hovered: None,
            highlighted: None,
            pressed: None,
            press_inside: false,
            captured: false,
            scroll: 0.0,
            body_top: 0.0,
            viewport: 0.0,
            content_height: 0.0,
            width: 0.0,
            height: 0.0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DropdownPanelWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.search_views(),
            &self.search_views(),
            &mut element.search,
            ctx,
            |view: &AnyView<State>| view,
            |_| None,
        );
        flags |= rebuild_children(
            &prev.loading_views(),
            &self.loading_views(),
            &mut element.busy,
            ctx,
            |view: &AnyView<State>| view,
            |_| None,
        );
        // Rows carry shaped runs and parsed icon paths, so they are rebuilt
        // only on a real content change — a rebuild that re-supplies the same
        // props keeps every cached layout.
        let rows_changed =
            prev.items != self.items || prev.selected != self.selected || prev.query != self.query;
        if rows_changed {
            element.rows = self.build_rows();
            element.reset_interaction();
            element.scroll = 0.0;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let body = self.body(&element.rows);
        if element.body != body {
            element.body = body;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.error != self.error || prev.empty_text != self.empty_text {
            element.message = Run::new(self.message());
            element.message_is_error = self.error.is_some();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.max_height != self.max_height {
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            if !self.open {
                element.reset_interaction();
                flags |= ChangeFlags::PAINT;
            }
        }
        element.items = self.items.clone();
        element.selected = self.selected.clone();
        element.multi = self.multi;
        element.max_selections = self.max_selections;
        element.anchor = self.anchor.clone();
        // Closures aren't comparable, so the adapters are reinstalled
        // unconditionally — cheap, and what every interactive widget does.
        element.on_change = erase_callback_arg(&self.on_change);
        element.on_open = self.on_open.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut DropdownPanelWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.search_views().iter().zip(element.search.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
        for (view, pod) in self.loading_views().iter().zip(element.busy.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// Everything a paint pass resolves from the theme once, then hands down to the
/// panel and each row — so a single pass reads the theme exactly once and no
/// two rows can disagree about a role or a radius.
struct Chrome {
    colors: DropdownColors,
    /// The level-2 elevation rung's `(blur, y_offset, color)`, if any.
    shadow: Option<(f64, f64, Color)>,
    container_radius: f64,
}

/// The type styles a panel's two text roles resolve to: `bodyLarge` for a row
/// label (`m3e_dropdown_menu_theme.dart:117`), `bodyMedium` for the empty and
/// error lines (`m3e_dropdown_menu_panel.dart:153`).
struct Styles {
    label: TextStyle,
    message: TextStyle,
}

impl Styles {
    /// Resolve both roles from `theme`, or from the M3 token literals when no
    /// theme is threaded into the pass.
    fn resolve(theme: Option<&Theme>) -> Self {
        let (mut label, mut message) = match theme {
            Some(theme) => (
                theme.type_scale.body_large.clone(),
                theme.type_scale.body_medium.clone(),
            ),
            None => (
                TextStyle::new(16.0, SHAPING_INK),
                TextStyle::new(14.0, SHAPING_INK),
            ),
        };
        label.color = SHAPING_INK;
        message.color = SHAPING_INK;
        Styles { label, message }
    }
}

impl Widget for DropdownPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let styles = Styles::resolve(Theme::from_layout_ctx(ctx));
        let cap = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            f64::INFINITY
        };

        // Width comes from the trigger, never from the content — the
        // reference's `SizedBox(width: renderBox.size.width)`. A never-painted
        // anchor falls back to the natural row width (see the module docs).
        let anchor_width = self.anchor.rect().width();
        self.width = if anchor_width > 0.0 {
            anchor_width
        } else {
            let mut natural: f64 = 0.0;
            for row in self.rows.iter_mut() {
                let mut width = row.label.natural_width(ctx, &styles.label);
                width += 2.0 * DROPDOWN_ITEM_H_PADDING;
                if row.check.is_some() {
                    width += DROPDOWN_ICON_GAP + DROPDOWN_ICON_SIZE;
                }
                natural = natural.max(width);
            }
            natural + 2.0 * DROPDOWN_CONTENT_PADDING
        };
        if cap.is_finite() {
            self.width = self.width.min(cap);
        }

        // The header: the search field, laid out across the panel's width
        // inside its own margins.
        let mut header = 0.0;
        if let Some(pod) = self.search.first_mut() {
            let width = (self.width - 2.0 * SEARCH_MARGIN_H).max(0.0);
            pod.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(width, SEARCH_FIELD_HEIGHT)),
            );
            pod.set_origin(Point::new(SEARCH_MARGIN_H, SEARCH_MARGIN_TOP));
            header = SEARCH_MARGIN_TOP + SEARCH_FIELD_HEIGHT + SEARCH_MARGIN_BOTTOM;
        }
        self.body_top = header + DROPDOWN_CONTENT_PADDING;

        // The body.
        let inner = (self.width - 2.0 * DROPDOWN_CONTENT_PADDING).max(0.0);
        self.content_height = match self.body {
            Body::Rows => {
                let text_width = |row: &Row| {
                    let mut width = inner - 2.0 * DROPDOWN_ITEM_H_PADDING;
                    if row.check.is_some() {
                        width -= DROPDOWN_ICON_GAP + DROPDOWN_ICON_SIZE;
                    }
                    width.max(0.0)
                };
                let mut y = 0.0;
                for i in 0..self.rows.len() {
                    if i > 0 {
                        y += DROPDOWN_ITEM_GAP;
                    }
                    let width = text_width(&self.rows[i]);
                    let label = self.rows[i].label.shape(ctx, &styles.label, width).height;
                    let mut height = label + 2.0 * DROPDOWN_ITEM_V_PADDING;
                    if self.rows[i].check.is_some() {
                        height = height.max(DROPDOWN_ICON_SIZE + 2.0 * DROPDOWN_ITEM_V_PADDING);
                    }
                    self.rows[i].top = y;
                    self.rows[i].height = height;
                    y += height;
                }
                y
            }
            Body::Loading => {
                let diameter = self.busy.first_mut().map_or(0.0, |pod| {
                    let size =
                        pod.layout_child(ctx, &BoxConstraints::loose(Size::new(inner, inner)));
                    size.height
                });
                diameter + 2.0 * DROPDOWN_LOADING_PADDING
            }
            Body::Error | Body::Empty => {
                let width = (inner - 2.0 * DROPDOWN_MESSAGE_PADDING).max(0.0);
                let height = self.message.shape(ctx, &styles.message, width).height;
                height + 2.0 * DROPDOWN_MESSAGE_PADDING
            }
        };

        let natural = self.body_top + self.content_height + DROPDOWN_CONTENT_PADDING;
        let mut cap_height = self.max_height;
        if bc.max().height.is_finite() {
            cap_height = cap_height.min(bc.max().height);
        }
        self.height = natural.min(cap_height);
        self.viewport = (self.height - self.body_top - DROPDOWN_CONTENT_PADDING).max(0.0);
        self.clamp_scroll();

        // The busy indicator is centered in whatever the body has left — read
        // after the viewport is resolved, which is why it is placed here rather
        // than in the `Body::Loading` arm that laid it out.
        let (width, body_top, viewport) = (self.width, self.body_top, self.viewport);
        if let Some(pod) = self.busy.first_mut() {
            let size = pod.size();
            pod.set_origin(Point::new(
                (width - size.width) / 2.0,
                body_top + (viewport - size.height) / 2.0,
            ));
        }

        bc.constrain(Size::new(self.width, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left this panel sends it
        // nothing, so the latched row is cleared from path membership.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = Chrome {
            colors: DropdownColors::resolve(theme),
            // The M3 menu rung, which the overlay seam already names — the
            // reference's `elevation: 3` / `M3EElevation.level2`.
            shadow: shadow(theme, OverlayElevation::Level2),
            container_radius: container_radius(theme),
        };
        let origin = ctx.origin();
        let size = Size::new(self.width, self.height);

        if let Some((blur, y_offset, color)) = chrome.shadow {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + y_offset),
                size,
                chrome.container_radius,
                blur,
                color,
            );
        }
        scene.fill_rounded_rect(
            origin,
            size,
            chrome.container_radius,
            chrome.colors.panel_container,
        );

        if let Some(pod) = self.search.first_mut() {
            pod.paint_child(ctx, scene);
        }

        match self.body {
            Body::Rows => {
                // Only a list that actually scrolls clips.
                let clipped = self.content_height > self.viewport + f64::EPSILON;
                if clipped {
                    scene.push_clip(
                        Point::new(origin.x, origin.y + self.body_top),
                        Size::new(self.width, self.viewport),
                    );
                }
                for i in 0..self.rows.len() {
                    self.paint_row(i, origin, &chrome, scene);
                }
                if clipped {
                    scene.pop_clip();
                }
            }
            Body::Loading => {
                if let Some(pod) = self.busy.first_mut() {
                    pod.paint_child(ctx, scene);
                }
            }
            Body::Error | Body::Empty => {
                let ink = if self.message_is_error {
                    chrome.colors.error
                } else {
                    chrome.colors.empty
                };
                self.message.paint(
                    Point::new(
                        origin.x + DROPDOWN_CONTENT_PADDING + DROPDOWN_MESSAGE_PADDING,
                        origin.y + self.body_top + DROPDOWN_MESSAGE_PADDING,
                    ),
                    ink,
                    scene,
                );
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.search.iter_mut().chain(self.busy.iter_mut()) {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if !self.open {
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            // The navigation keys are the panel's; everything else (typing,
            // caret motion inside the search field) falls through to the
            // focused pod below.
            if self.handle_key(ctx, &key.key) == EventResult::Handled {
                return EventResult::Handled;
            }
            if let Some(pod) = self.search.first_mut() {
                return route_event_single(pod, ctx, event);
            }
            return EventResult::Ignored;
        }
        // The search field owns any pointer landing inside it.
        if let Some(pod) = self.search.first_mut()
            && (pod.is_active() || pod.contains(event.position()))
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }
        match event {
            InputEvent::Scroll { position, delta } => self.handle_scroll(ctx, *position, delta),
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let rows = &self.rows;
        let search = &self.search;
        ctx.push_container(
            Role::ListBox,
            |_node| {},
            |ctx| {
                if let Some(pod) = search.first() {
                    pod.semantics_child(ctx);
                }
                for row in rows.iter() {
                    // Per-row bounds would need a `ChildPod` per row to descend
                    // through; rows are painted internally, so every node below
                    // shares the whole panel's bounds — the same v1 limitation
                    // `crate::menu`'s own panel accepts.
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.label.content.as_str());
                        node.set_selected(row.selected);
                        if row.enabled {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }

    visit_children!(search, busy);
}

impl DropdownPanelWidget {
    /// The corner radii row `i` takes, resolved through the card-list treatment
    /// the reference's own item style points at (`_buildEffectiveRadius` +
    /// `_calculateBaseRadius`).
    fn row_radii(&self, i: usize) -> CornerRadii {
        let row = &self.rows[i];
        let pressed = self.pressed == Some(i) && self.press_inside;
        let hovered = self.hovered == Some(i) || self.highlighted == Some(i);
        let current = if pressed {
            DROPDOWN_ITEM_PRESSED_RADIUS
        } else if hovered {
            DROPDOWN_ITEM_HOVER_RADIUS
        } else if row.selected {
            DROPDOWN_ITEM_OUTER_RADIUS
        } else {
            DROPDOWN_ITEM_INNER_RADIUS
        };
        if row.selected {
            // The reference's first branch short-circuits: a selected row loses
            // its cap corners entirely.
            return CornerRadii::uniform(current);
        }
        card_radii(
            card_position(i, self.rows.len()),
            DROPDOWN_ITEM_OUTER_RADIUS,
            current,
        )
    }

    /// Paint row `i`: its card, its state layer, its label and its check.
    fn paint_row(&self, i: usize, origin: Point, chrome: &Chrome, scene: &mut dyn PaintScene) {
        let row = &self.rows[i];
        let colors = &chrome.colors;
        let box_origin = Point::new(
            origin.x + DROPDOWN_CONTENT_PADDING,
            origin.y + self.body_top + row.top - self.scroll,
        );
        let box_size = Size::new(
            (self.width - 2.0 * DROPDOWN_CONTENT_PADDING).max(0.0),
            row.height,
        );
        let radii = self.row_radii(i);
        scene.fill_rounded_rect_radii(
            box_origin,
            box_size,
            radii,
            colors.item_background(row.enabled, row.selected),
        );

        let mut state = InteractionState::new();
        state.hovered = self.hovered == Some(i);
        state.focused = self.highlighted == Some(i);
        state.pressed = self.pressed == Some(i) && self.press_inside;
        let opacity = if row.enabled {
            state.resolve_opacity()
        } else {
            0.0
        };
        if opacity > 0.0 {
            scene.fill_rounded_rect_radii(
                box_origin,
                box_size,
                radii,
                with_alpha(colors.item_foreground(row.enabled, row.selected), opacity),
            );
        }

        let ink = colors.item_foreground(row.enabled, row.selected);
        let middle = box_origin.y + box_size.height / 2.0;
        let label_size = row.label.size();
        row.label.paint(
            Point::new(
                box_origin.x + DROPDOWN_ITEM_H_PADDING,
                middle - label_size.height / 2.0,
            ),
            ink,
            scene,
        );
        if let Some(check) = &row.check {
            check.paint(
                Point::new(
                    box_origin.x + box_size.width - DROPDOWN_ITEM_H_PADDING - check.extent,
                    middle - check.extent / 2.0,
                ),
                ink,
                scene,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dropdown::dropdown_item;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BezPath, Brush, KeyEvent, Modifiers, PointerButton, text::TextContext};
    use frust::{Brightness, FrameTime};
    use std::any::Any;

    const AREA: Size = Size::new(600.0, 800.0);
    /// The width the harness's anchor advertises — what the panel must match.
    const ANCHOR_WIDTH: f64 = 240.0;

    /// What the panel reported, in order.
    #[derive(Default)]
    struct AppState {
        selections: Vec<Vec<String>>,
        opens: Vec<bool>,
        queries: Vec<String>,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        radii: Vec<CornerRadii>,
        paths: Vec<Point>,
        runs: usize,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn fill_rounded_rect_radii(
            &mut self,
            origin: Point,
            size: Size,
            radii: CornerRadii,
            color: Color,
        ) {
            self.radii.push(radii);
            self.rrects.push((origin, size, radii.largest(), color));
        }
        fn fill_path(&mut self, origin: Point, _path: &BezPath, _brush: &Brush) {
            self.paths.push(origin);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.runs += 1;
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _std: f64, _c: Color) {}
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn pop_clip(&mut self) {}
    }

    fn fruit() -> Vec<DropdownItem> {
        vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana"),
            dropdown_item("Cherry", "cherry"),
        ]
    }

    fn anchor() -> OverlayAnchor {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 40.0, 20.0 + ANCHOR_WIDTH, 92.0));
        anchor
    }

    fn view(items: Vec<DropdownItem>, anchor: &OverlayAnchor) -> DropdownPanelView<AppState> {
        dropdown_panel(items, |state: &mut AppState, values| {
            state.selections.push(values)
        })
        .anchor(anchor)
        .on_open(|state: &mut AppState, open| state.opens.push(open))
        .on_query(|state: &mut AppState, query| state.queries.push(query))
    }

    fn build(view: &DropdownPanelView<AppState>) -> DropdownPanelWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DropdownPanelWidget, area: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(area))
    }

    /// Build + lay out in one step: the state every event/paint test starts in.
    fn ready(view: &DropdownPanelView<AppState>) -> DropdownPanelWidget {
        let mut w = build(view);
        layout(&mut w, AREA);
        w
    }

    fn rebuild(
        w: &mut DropdownPanelWidget,
        prev: &DropdownPanelView<AppState>,
        next: &DropdownPanelView<AppState>,
    ) {
        let mut counter = 0u64;
        View::<AppState>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn paint(w: &mut DropdownPanelWidget) -> Recorder {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut rec = Recorder::default();
        let size = Size::new(w.width, w.height);
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut DropdownPanelWidget,
        state: &mut AppState,
        event: &InputEvent,
    ) -> EventResult {
        let any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any, Point::ZERO, Size::new(w.width, w.height));
        w.event(&mut ctx, event)
    }

    /// The centre of row `row`, in panel-local coordinates.
    fn row_center(w: &DropdownPanelWidget, row: usize) -> Point {
        let (top, height) = w.band(row);
        Point::new(w.width / 2.0, w.body_top + top + height / 2.0 - w.scroll)
    }

    fn click(w: &mut DropdownPanelWidget, state: &mut AppState, row: usize) {
        let c = row_center(w, row);
        dispatch(w, state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(w, state, &pointer(PointerPhase::Up, c.x, c.y));
    }

    // ---- structure --------------------------------------------------------

    #[test]
    fn the_panel_matches_the_trigger_s_width() {
        let a = anchor();
        let w = ready(&view(fruit(), &a));
        assert_eq!(w.width(), ANCHOR_WIDTH);
    }

    #[test]
    fn a_never_painted_anchor_falls_back_to_the_natural_content_width() {
        let bare = OverlayAnchor::new();
        let w = ready(&view(fruit(), &bare));
        assert!(
            w.width() > 2.0 * DROPDOWN_CONTENT_PADDING,
            "a degenerate anchor still sizes the panel from its rows"
        );
        assert!(w.width() <= AREA.width);
    }

    #[test]
    fn every_row_gets_a_band_and_the_gap_sits_between_them() {
        let a = anchor();
        let w = ready(&view(fruit(), &a));
        assert_eq!(w.rows.len(), 3);
        for i in 1..w.rows.len() {
            let (prev_top, prev_height) = w.band(i - 1);
            let (top, _) = w.band(i);
            assert!((top - (prev_top + prev_height) - DROPDOWN_ITEM_GAP).abs() < 1e-6);
        }
    }

    #[test]
    fn a_tall_list_caps_at_the_max_height_and_clips_while_scrolling() {
        let a = anchor();
        let many: Vec<DropdownItem> = (0..40)
            .map(|i| dropdown_item(format!("Item {i}"), format!("v{i}")))
            .collect();
        let mut w = ready(&view(many, &a));
        assert_eq!(w.height(), DROPDOWN_PANEL_MAX_HEIGHT);
        assert!(w.content_height > w.viewport);
        let rec = paint(&mut w);
        assert_eq!(rec.clips, 1, "a scrolling list clips its own viewport");

        let mut state = AppState::default();
        let inside = Point::new(w.width / 2.0, w.body_top + 10.0);
        dispatch(
            &mut w,
            &mut state,
            &InputEvent::Scroll {
                position: inside,
                delta: ScrollDelta::Pixels(0.0, 40.0),
            },
        );
        assert_eq!(w.scroll(), 40.0);
    }

    #[test]
    fn a_short_list_never_clips() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        assert!(w.height() < DROPDOWN_PANEL_MAX_HEIGHT);
        let rec = paint(&mut w);
        assert_eq!(rec.clips, 0);
    }

    #[test]
    fn the_search_field_is_a_header_above_the_scrolled_list() {
        let a = anchor();
        let plain = ready(&view(fruit(), &a));
        let searchable = ready(&view(fruit(), &a).searchable(true));
        assert!(searchable.body_top > plain.body_top);
        let rect = searchable.search_rect().expect("a mounted search field");
        assert_eq!(rect.x0, SEARCH_MARGIN_H);
        assert_eq!(rect.y0, SEARCH_MARGIN_TOP);
        assert!(
            rect.y1 <= searchable.body_top,
            "the header never overlaps the list"
        );
    }

    // ---- the filter -------------------------------------------------------

    #[test]
    fn a_query_narrows_the_rows_the_panel_renders() {
        let a = anchor();
        let w = ready(&view(fruit(), &a).query("an"));
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.rows[0].value, "banana");
    }

    #[test]
    fn a_query_matching_nothing_shows_the_empty_message() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).query("zzz"));
        assert_eq!(w.body, Body::Empty);
        assert!(w.rows.is_empty());
        let rec = paint(&mut w);
        assert!(rec.runs > 0, "the empty line is painted");
        assert!(rec.radii.is_empty(), "no row cards at all");
    }

    #[test]
    fn a_rebuilt_query_re_filters_the_rows_and_drops_the_highlight() {
        let a = anchor();
        let prev = view(fruit(), &a);
        let mut w = ready(&prev);
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.highlighted(), Some(0));

        let next = view(fruit(), &a).query("cher");
        rebuild(&mut w, &prev, &next);
        layout(&mut w, AREA);
        assert_eq!(w.rows.len(), 1);
        assert_eq!(w.rows[0].value, "cherry");
        assert_eq!(w.highlighted(), None);
    }

    // ---- single select ----------------------------------------------------

    #[test]
    fn a_single_select_pick_reports_one_value_and_closes() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        assert_eq!(state.selections, vec![vec!["banana".to_string()]]);
        assert_eq!(state.opens, vec![false]);
    }

    #[test]
    fn a_single_select_never_flips_its_own_state() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        // Nothing fed back down, so nothing is selected yet.
        assert!(w.rows.iter().all(|row| !row.selected));
    }

    #[test]
    fn re_picking_the_selected_option_clears_it_the_way_toggle_only_does() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).selected(vec!["banana".to_string()]));
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        assert_eq!(state.selections, vec![Vec::<String>::new()]);
        assert_eq!(state.opens, vec![false]);
    }

    #[test]
    fn a_single_select_pick_replaces_whatever_was_selected() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).selected(vec!["apple".to_string()]));
        let mut state = AppState::default();
        click(&mut w, &mut state, 2);
        assert_eq!(state.selections, vec![vec!["cherry".to_string()]]);
    }

    // ---- multi select -----------------------------------------------------

    #[test]
    fn a_multi_select_pick_adds_in_item_order_and_stays_open() {
        let a = anchor();
        let mut w = ready(
            &view(fruit(), &a)
                .multi(true)
                .selected(vec!["cherry".to_string()]),
        );
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        assert_eq!(
            state.selections,
            vec![vec!["apple".to_string(), "cherry".to_string()]],
            "item order, not click order"
        );
        assert!(state.opens.is_empty(), "a multi-select pick never closes");
    }

    #[test]
    fn a_multi_select_pick_on_a_selected_row_removes_it() {
        let a = anchor();
        let mut w = ready(
            &view(fruit(), &a)
                .multi(true)
                .selected(vec!["apple".to_string(), "cherry".to_string()]),
        );
        let mut state = AppState::default();
        click(&mut w, &mut state, 0);
        assert_eq!(state.selections, vec![vec!["cherry".to_string()]]);
    }

    #[test]
    fn max_selections_refuses_a_new_pick_but_never_a_deselect() {
        let a = anchor();
        let mut w = ready(
            &view(fruit(), &a)
                .multi(true)
                .max_selections(1)
                .selected(vec!["apple".to_string()]),
        );
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        assert!(state.selections.is_empty(), "the cap refuses the addition");
        click(&mut w, &mut state, 0);
        assert_eq!(
            state.selections,
            vec![Vec::<String>::new()],
            "a deselect is never capped"
        );
    }

    #[test]
    fn a_disabled_row_reports_nothing_at_all() {
        let a = anchor();
        let items = vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana").disabled(true),
        ];
        let mut w = ready(&view(items, &a));
        let mut state = AppState::default();
        click(&mut w, &mut state, 1);
        assert!(state.selections.is_empty());
        assert!(state.opens.is_empty(), "not even a close");
    }

    #[test]
    fn a_release_outside_the_armed_row_reports_nothing() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        let down = row_center(&w, 0);
        let up = row_center(&w, 2);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, down.x, down.y),
        );
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, up.x, up.y));
        assert!(state.selections.is_empty());
    }

    #[test]
    fn only_a_primary_press_arms_a_row() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: c,
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, c.x, c.y));
        assert!(state.selections.is_empty());
    }

    #[test]
    fn a_cancel_clears_the_press_without_reporting() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Cancel, c.x, c.y));
        assert!(state.selections.is_empty());
        assert_eq!(w.pressed, None);
    }

    #[test]
    fn a_press_on_the_panel_s_own_background_is_left_for_the_host() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        // The panel's bottom padding band, below the last row.
        let y = w.height() - 1.0;
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 5.0, y)),
            EventResult::Ignored
        );
    }

    // ---- keyboard ---------------------------------------------------------

    #[test]
    fn arrow_down_enters_at_the_top_and_arrow_up_at_the_bottom() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.highlighted(), Some(0));

        let mut fresh = ready(&view(fruit(), &a));
        dispatch(&mut fresh, &mut state, &key(NamedKey::ArrowUp));
        assert_eq!(fresh.highlighted(), Some(2));
    }

    #[test]
    fn the_highlight_skips_disabled_rows_and_stops_at_the_ends() {
        let a = anchor();
        let items = vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana").disabled(true),
            dropdown_item("Cherry", "cherry"),
        ];
        let mut w = ready(&view(items, &a));
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.highlighted(), Some(0));
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.highlighted(), Some(2), "the disabled row is skipped");
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(w.highlighted(), Some(2), "the end does not wrap");
    }

    #[test]
    fn enter_activates_the_highlighted_row() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        assert_eq!(
            dispatch(&mut w, &mut state, &key(NamedKey::Enter)),
            EventResult::Handled
        );
        assert_eq!(state.selections, vec![vec!["banana".to_string()]]);
    }

    #[test]
    fn enter_with_no_highlight_reports_nothing() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &key(NamedKey::Enter)),
            EventResult::Ignored
        );
        assert!(state.selections.is_empty());
    }

    #[test]
    fn escape_is_left_for_the_host_to_dismiss_on() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &key(NamedKey::Escape)),
            EventResult::Ignored
        );
    }

    #[test]
    fn the_highlight_scrolls_its_row_into_view() {
        let a = anchor();
        let many: Vec<DropdownItem> = (0..40)
            .map(|i| dropdown_item(format!("Item {i}"), format!("v{i}")))
            .collect();
        let mut w = ready(&view(many, &a));
        let mut state = AppState::default();
        for _ in 0..12 {
            dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown));
        }
        assert_eq!(w.highlighted(), Some(11));
        assert!(w.scroll() > 0.0, "the highlight pulled the list down");
        let (top, height) = w.band(11);
        assert!(top >= w.scroll() && top + height <= w.scroll() + w.viewport);
    }

    #[test]
    fn a_closed_panel_ignores_every_interaction() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).open(false));
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y)),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, &mut state, &key(NamedKey::ArrowDown)),
            EventResult::Ignored
        );
        assert!(state.selections.is_empty());
    }

    // ---- async seam -------------------------------------------------------

    #[test]
    fn loading_replaces_the_rows_with_the_busy_indicator() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).loading(true));
        assert_eq!(w.body, Body::Loading);
        assert_eq!(w.busy.len(), 1, "the indicator pod is mounted");
        let rec = paint(&mut w);
        assert!(rec.radii.is_empty(), "no row cards while loading");
    }

    #[test]
    fn a_loading_panel_reports_nothing_on_a_press() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).loading(true));
        let mut state = AppState::default();
        let middle = Point::new(w.width() / 2.0, w.body_top + w.viewport / 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, middle.x, middle.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, middle.x, middle.y),
        );
        assert!(state.selections.is_empty());
    }

    #[test]
    fn loading_outranks_an_empty_list_and_an_error_outranks_both() {
        let a = anchor();
        let loading = ready(&view(Vec::new(), &a).loading(true));
        assert_eq!(loading.body, Body::Loading);
        let error = ready(&view(Vec::new(), &a).error(Some("boom".to_string())));
        assert_eq!(error.body, Body::Error);
        let empty = ready(&view(Vec::new(), &a));
        assert_eq!(empty.body, Body::Empty);
    }

    #[test]
    fn a_load_that_settles_swaps_the_busy_row_for_the_options() {
        let a = anchor();
        let prev = view(Vec::new(), &a).loading(true);
        let mut w = ready(&prev);
        assert_eq!(w.busy.len(), 1);

        let next = view(fruit(), &a).loading(false);
        rebuild(&mut w, &prev, &next);
        layout(&mut w, AREA);
        assert_eq!(w.body, Body::Rows);
        assert_eq!(w.busy.len(), 0, "the indicator pod is unmounted");
        assert_eq!(w.rows.len(), 3);
    }

    #[test]
    fn an_error_paints_its_message_in_the_error_role() {
        let a = anchor();
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut w = ready(&view(fruit(), &a).error(Some("network down".to_string())));
        assert!(w.message_is_error);
        let rec = paint(&mut w);
        assert!(rec.runs > 0);
        // The panel container is the one rounded rect an error body paints.
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_highest);
    }

    // ---- paint ------------------------------------------------------------

    #[test]
    fn a_selected_row_takes_the_secondary_container_fill_and_a_trailing_check() {
        let a = anchor();
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut w = ready(&view(fruit(), &a).selected(vec!["banana".to_string()]));
        let rec = paint(&mut w);
        let selected_fill = theme.scheme().secondary_container;
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, color)| *color == selected_fill),
            "the selected row fills with `secondaryContainer`"
        );
        assert_eq!(rec.paths.len(), 1, "exactly one check glyph, on row 1");
    }

    #[test]
    fn a_selected_row_loses_its_cap_corners_the_way_the_reference_does() {
        let a = anchor();
        // Row 0 is both `First` (capped) and selected — the reference's
        // short-circuit makes it uniform instead.
        let w = ready(&view(fruit(), &a).selected(vec!["apple".to_string()]));
        assert_eq!(
            w.row_radii(0),
            CornerRadii::uniform(DROPDOWN_ITEM_OUTER_RADIUS)
        );
        assert_eq!(
            w.row_radii(1),
            card_radii(
                card_position(1, 3),
                DROPDOWN_ITEM_OUTER_RADIUS,
                DROPDOWN_ITEM_INNER_RADIUS
            )
        );
    }

    #[test]
    fn the_card_list_treatment_caps_the_first_and_last_rows() {
        let a = anchor();
        let w = ready(&view(fruit(), &a));
        assert_eq!(
            w.row_radii(0),
            CornerRadii::new(
                DROPDOWN_ITEM_OUTER_RADIUS,
                DROPDOWN_ITEM_OUTER_RADIUS,
                DROPDOWN_ITEM_INNER_RADIUS,
                DROPDOWN_ITEM_INNER_RADIUS
            )
        );
        assert_eq!(
            w.row_radii(2),
            CornerRadii::new(
                DROPDOWN_ITEM_INNER_RADIUS,
                DROPDOWN_ITEM_INNER_RADIUS,
                DROPDOWN_ITEM_OUTER_RADIUS,
                DROPDOWN_ITEM_OUTER_RADIUS
            )
        );
        // A lone row is capped on every corner.
        let single = ready(&view(vec![dropdown_item("Only", "only")], &a));
        assert_eq!(
            single.row_radii(0),
            CornerRadii::uniform(DROPDOWN_ITEM_OUTER_RADIUS)
        );
    }

    #[test]
    fn hover_and_press_morph_the_inner_radius() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        let c = row_center(&w, 1);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        assert_eq!(
            w.row_radii(1),
            CornerRadii::uniform(DROPDOWN_ITEM_HOVER_RADIUS)
        );
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, c.x, c.y));
        assert_eq!(
            w.row_radii(1),
            CornerRadii::uniform(DROPDOWN_ITEM_PRESSED_RADIUS)
        );
    }

    #[test]
    fn a_disabled_row_never_washes() {
        let a = anchor();
        let items = vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana").disabled(true),
        ];
        let mut w = ready(&view(items, &a));
        let mut state = AppState::default();
        let c = row_center(&w, 1);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        assert_eq!(w.hovered, None, "a disabled row is never latched");
    }

    #[test]
    fn a_paint_with_no_hover_link_clears_the_latched_row() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a));
        let mut state = AppState::default();
        let c = row_center(&w, 0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, c.x, c.y));
        assert_eq!(w.hovered, Some(0));
        // `PaintCtx::for_test` reports no hover path.
        paint(&mut w);
        assert_eq!(w.hovered, None);
    }

    #[test]
    fn every_painted_rect_stays_inside_the_panel_s_own_box() {
        let a = anchor();
        let mut w = ready(&view(fruit(), &a).selected(vec!["banana".to_string()]));
        let width = w.width();
        let height = w.height();
        let rec = paint(&mut w);
        for (origin, size, _, _) in rec.rrects {
            assert!(origin.x >= 0.0 && origin.x + size.width <= width + 1e-6);
            assert!(origin.y >= 0.0 && origin.y + size.height <= height + 1e-6);
        }
    }

    #[test]
    fn a_rebuild_that_re_supplies_the_same_props_keeps_its_shaped_runs() {
        let a = anchor();
        let prev = view(fruit(), &a);
        let mut w = ready(&prev);
        let before: Vec<Size> = w.rows.iter().map(|row| row.label.size()).collect();
        let next = view(fruit(), &a);
        rebuild(&mut w, &prev, &next);
        let after: Vec<Size> = w.rows.iter().map(|row| row.label.size()).collect();
        assert_eq!(before, after);
        assert!(after.iter().all(|size| size.width > 0.0));
    }

    #[test]
    fn the_semantics_tree_is_a_listbox_of_selectable_options() {
        use frust_core::RenderRoot;
        let a = anchor();
        let mut root: RenderRoot<AppState, DropdownPanelView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let cell = a.clone();
        let mut logic =
            move |_s: &mut AppState| view(fruit(), &cell).selected(vec!["banana".to_string()]);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);
        let tree = root.semantics();
        assert!(
            tree.nodes
                .iter()
                .any(|(_, node)| node.role() == Role::ListBox)
        );
        let options: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == Role::ListBoxOption)
            .collect();
        assert_eq!(options.len(), 3);
        assert_eq!(
            options
                .iter()
                .filter(|(_, n)| n.is_selected() == Some(true))
                .count(),
            1
        );
    }
}
