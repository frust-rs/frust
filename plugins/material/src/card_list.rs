// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/lists/ (`m3e_lists.dart`'s `M3ECardList`,
//   `components/m3e_card_list_item.dart`'s `calculateCardPosition`/
//   `calculateCardRadius`, `styles/m3e_list_theme.dart`'s
//   `M3EListCardListTheme`, `enums/m3e_list_enums.dart`'s `M3ECardPosition`)
// The `.builder` (lazy `ListView`) constructor, `M3ECardRadiusMotion`'s
// radius spring, `onLongPress`, and the per-index `colorBuilder`/
// `borderRadiusBuilder` escape hatches stay unported — see the module docs'
// Not ported section.

//! The M3E **card list**: a vertical stack of card-backed rows whose corner
//! radii vary by position — the first and last cards keep a large *outer*
//! radius on their outward edges, while every adjoining edge takes a small
//! *inner* radius, so a run of cards reads as one grouped surface.
//!
//! # Corner matrix
//!
//! [`card_position`] classifies each index and [`card_radii`] resolves it into
//! a [`frust::authoring::CornerRadii`] (`calculateCardPosition` /
//! `calculateCardRadius`):
//!
//! | Position | top-left / top-right | bottom-right / bottom-left |
//! |---|---|---|
//! | [`CardPosition::Single`] | outer | outer |
//! | [`CardPosition::First`] | outer | inner |
//! | [`CardPosition::Middle`] | inner | inner |
//! | [`CardPosition::Last`] | inner | outer |
//!
//! Defaults are the reference's own (`M3EListCardListTheme`'s
//! `defaultOuterRadius`/`defaultInnerRadius`/`defaultGap`/
//! `defaultItemPadding`): [`CARD_LIST_OUTER_RADIUS`] 24dp,
//! [`CARD_LIST_INNER_RADIUS`] 4dp, [`CARD_LIST_GAP`] 4dp between cards, and
//! [`CARD_LIST_ITEM_PADDING`] 12dp inside each one.
//!
//! # Children
//!
//! [`card_list`] takes arbitrary [`frust::authoring::AnyView`] children, the
//! way upstream's `itemBuilder` does. [`card_list_items`] is the typed
//! convenience for the common case — a list of [`super::list_item::ListItem`]
//! rows, erased for you.
//!
//! A row needs no marking to be hosted: a [`super::list_item::ListItem`]
//! paints no container of its own unless asked
//! ([`super::list_item::ListItem::contained`]), which is exactly the state
//! upstream's `M3EListItemScope` puts a hosted row in — the card below it is
//! the only surface. Passing an explicitly `contained` row into this stack
//! doubles the surfaces up and is the one combination to avoid.
//!
//! # Surfaces and states
//!
//! Each card paints the filled-card role (`colors.surface_container_highest`,
//! `M3EListCardListTheme.backgroundColor`), or the selection fill
//! (`colors.secondary_container`, the `m3eSelectionFill` a selection scope
//! contributes upstream) for an index passed to [`CardListView::selected`]; an
//! explicit [`CardListView::color`] replaces the unselected fill.
//!
//! [`CardListView::on_press`] makes the whole stack interactive with a
//! per-index callback: the list owns capture (the same fire-on-up-inside
//! contract [`mod@super::card`] and [`mod@super::list_item`] follow), tracks
//! which card is armed, and paints that card's
//! [`crate::interaction::InteractionState`] overlay at *its* radii, tinted
//! `on_surface`. Hover follows the catalog's three-part contract
//! (claim → latch → self-correct in paint) and is **enabled-gated**. Without
//! `on_press` the list is a plain container that routes pointer events into
//! its children, so a row's own [`super::list_item::ListItem::on_press`] (or a
//! trailing control) stays live.
//!
//! [`CardListView::enabled`]`(false)` dims every card to `on_surface` at
//! [`crate::interaction::DISABLED_CONTAINER_OPACITY`], claims no hover, paints
//! no state layer, fires nothing, and — mirroring `card`'s own disabled
//! early-return — routes no pointer event into its children either.
//!
//! # Semantics
//!
//! The stack contributes one [`Role::List`] container node whose accesskit
//! children are the rows' own nodes (a hosted
//! [`super::list_item::ListItem`] contributes its `Role::ListItem` node with
//! *its* bounds). An interactive list carries [`Action::Click`] on that
//! container node, and `Node::set_disabled()` instead while disabled:
//! [`frust::authoring::SemanticsCtx`] exposes no seam for contributing a node
//! at a *child's* bounds, so a synthetic per-card node would publish the whole
//! stack's rect for every row — worse than one honest container node. An app
//! that needs per-row actions in the accessibility tree puts `on_press` on
//! each [`super::list_item::ListItem`] instead of on the list.
//!
//! # Not ported
//!
//! * `M3ECardList.builder` — the lazy `ListView.builder` variant (and its
//!   `controller`/`physics`/`shrinkWrap`/`listPadding` knobs). This container
//!   materializes every child, like the default `Column` constructor;
//!   [`frust::list_view`] is the framework's lazy list.
//! * `M3ECardRadiusMotion` — upstream springs a card's radii
//!   (`expressiveSpatialDefault`) whenever its position changes. This port
//!   applies the resolved radii directly, the same shape upstream's own
//!   `snap: true` path takes: a card's position only changes on a structural
//!   rebuild, and a per-card spring controller set is out of this module's
//!   scope. Documented rather than silently dropped — see the same
//!   simplification note [`mod@super::fab`] carries for its hover elevation.
//! * `onLongPress` — no long-press gesture primitive exists in this crate yet
//!   (the v1 scope line [`mod@super::button`] and [`mod@super::card`] both
//!   draw).
//! * The per-index `colorBuilder`/`borderRadiusBuilder`, `mouseCursor`,
//!   `semanticLabelBuilder`, `emptyBuilder`, `variant`/`border` and
//!   `margin`/`listPadding` escape hatches — the same customization surface
//!   `card` leaves unported. [`CardListView::color`]/
//!   [`CardListView::selected`] cover the fills those builders are reached
//!   for in practice.

use std::collections::BTreeSet;
use std::rc::Rc;

use frust::Theme;
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CornerRadii, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx,
    View, Widget,
};
use kurbo::{Point, Size};
use peniko::Color;

use crate::interaction::{
    DISABLED_CONTAINER_OPACITY, HapticSignal, InteractionState, MaterialHaptics,
};
use crate::list_item::ListItem;

use super::press::presses;

/// Outward corner radius of the first/last (and only) card
/// (`M3EListCardListTheme.defaultOuterRadius`, 24dp).
pub const CARD_LIST_OUTER_RADIUS: f64 = 24.0;
/// Corner radius of every adjoining card edge
/// (`M3EListCardListTheme.defaultInnerRadius`, 4dp).
pub const CARD_LIST_INNER_RADIUS: f64 = 4.0;
/// Vertical gap between adjacent cards
/// (`M3EListCardListTheme.defaultGap`, 4dp).
pub const CARD_LIST_GAP: f64 = 4.0;
/// Content padding inside each card
/// (`M3EListCardListTheme.defaultItemPadding`, `EdgeInsets.all(12)`).
pub const CARD_LIST_ITEM_PADDING: f64 = 12.0;

/// Unthemed-fallback card fill (a theme resolves this from
/// `colors.surface_container_highest`).
const FILLED_CONTAINER: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback selected-card fill (a theme resolves this from
/// `colors.secondary_container`).
const SELECTED_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback state-layer content color (a theme resolves this from
/// `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);

/// A card's place in the stack, which decides its corner radii
/// (`M3ECardPosition`). See the [module docs](self)' corner matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardPosition {
    /// The first card of a stack holding more than one.
    First,
    /// A card between the first and the last.
    Middle,
    /// The last card of a stack holding more than one.
    Last,
    /// The only card in the stack.
    Single,
}

/// Classify index `index` of a `total`-card stack (`calculateCardPosition`).
/// An empty stack has no positions to classify; `index >= total` is treated as
/// the last card rather than panicking, since a caller can only reach it
/// mid-rebuild.
pub fn card_position(index: usize, total: usize) -> CardPosition {
    if total <= 1 {
        CardPosition::Single
    } else if index == 0 {
        CardPosition::First
    } else if index + 1 >= total {
        CardPosition::Last
    } else {
        CardPosition::Middle
    }
}

/// The corner radii for `position` (`calculateCardRadius`) — see the
/// [module docs](self)' corner matrix.
pub fn card_radii(position: CardPosition, outer: f64, inner: f64) -> CornerRadii {
    match position {
        // `CornerRadii::new` is clockwise from the top-left.
        CardPosition::Single => CornerRadii::uniform(outer),
        CardPosition::First => CornerRadii::new(outer, outer, inner, inner),
        CardPosition::Last => CornerRadii::new(inner, inner, outer, outer),
        CardPosition::Middle => CornerRadii::uniform(inner),
    }
}

/// A view-held, typed per-index press callback (erased on build).
type OnPressIndex<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative M3E card list. See the [module docs](self).
pub struct CardListView<State: 'static> {
    items: Vec<AnyView<State>>,
    outer_radius: f64,
    inner_radius: f64,
    gap: f64,
    item_padding: f64,
    color: Option<Color>,
    selected: BTreeSet<usize>,
    enabled: bool,
    haptic: HapticSignal,
    on_press: Option<OnPressIndex<State>>,
}

/// Stack `items` as a card list, each child painted inside its own card
/// surface at its position's radii (upstream's arbitrary `itemBuilder`
/// children).
pub fn card_list<State: 'static>(
    items: impl IntoIterator<Item = AnyView<State>>,
) -> CardListView<State> {
    CardListView {
        items: items.into_iter().collect(),
        outer_radius: CARD_LIST_OUTER_RADIUS,
        inner_radius: CARD_LIST_INNER_RADIUS,
        gap: CARD_LIST_GAP,
        item_padding: CARD_LIST_ITEM_PADDING,
        color: None,
        selected: BTreeSet::new(),
        enabled: true,
        haptic: HapticSignal::None,
        on_press: None,
    }
}

/// Stack [`ListItem`] rows as a card list — the typed convenience over
/// [`card_list`]. A row paints no container of its own by default, so the
/// stack's card is the only surface (upstream's `M3EListItemScope` effect);
/// see the [module docs](self)' Children section.
pub fn card_list_items<State: 'static>(
    items: impl IntoIterator<Item = ListItem<State>>,
) -> CardListView<State> {
    card_list(items.into_iter().map(frust::authoring::any))
}

impl<State: 'static> CardListView<State> {
    /// Override the outward corner radius of the first/last (and only) card.
    /// Defaults to [`CARD_LIST_OUTER_RADIUS`].
    pub fn outer_radius(mut self, radius: f64) -> Self {
        self.outer_radius = radius;
        self
    }

    /// Override the radius of every adjoining card edge. Defaults to
    /// [`CARD_LIST_INNER_RADIUS`].
    pub fn inner_radius(mut self, radius: f64) -> Self {
        self.inner_radius = radius;
        self
    }

    /// Override the vertical gap between adjacent cards. Defaults to
    /// [`CARD_LIST_GAP`].
    pub fn gap(mut self, gap: f64) -> Self {
        self.gap = gap;
        self
    }

    /// Override the content padding inside each card. Defaults to
    /// [`CARD_LIST_ITEM_PADDING`].
    pub fn item_padding(mut self, padding: f64) -> Self {
        self.item_padding = padding;
        self
    }

    /// Replace the unselected card fill (`M3ECardList.color`). A selected
    /// index still takes the selection fill.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Mark these indices selected — each takes the selection fill
    /// (`colors.secondary_container`).
    pub fn selected(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        self.selected = indices.into_iter().collect();
        self
    }

    /// Gate the interactive treatment (hover/press tint, firing
    /// [`CardListView::on_press`]/[`CardListView::haptic`]) and dim every
    /// card. Defaults to `true`; only meaningful once `on_press` is chained.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the haptic fired immediately before [`CardListView::on_press`] on a
    /// successful release (`M3ECardList.haptic`). Defaults to
    /// [`HapticSignal::None`] (no feedback).
    pub fn haptic(mut self, haptic: HapticSignal) -> Self {
        self.haptic = haptic;
        self
    }

    /// Make every card interactive, firing `on_press` with the released
    /// card's index (`M3ECardList.onTap`). See the [module docs](self)'
    /// Surfaces and states section.
    pub fn on_press<F: Fn(&mut State, usize) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }
}

/// The retained widget for a [`CardListView`].
pub struct CardListWidget {
    items: Vec<ChildPod>,
    outer_radius: f64,
    inner_radius: f64,
    gap: f64,
    item_padding: f64,
    color: Option<Color>,
    selected: BTreeSet<usize>,
    interactive: bool,
    enabled: bool,
    haptic: HapticSignal,
    /// Each card's `(y, height)` in local space, recorded during layout — the
    /// paint rects and the event pass's hit test both read this.
    cards: Vec<(f64, f64)>,
    /// The laid-out stack width; the card rects span it fully
    /// (`M3ECard(width: double.infinity)`).
    width: f64,
    /// The card index a `Down` armed (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`/loss of interactivity or enablement.
    armed: Option<usize>,
    /// The card currently painting a state-layer overlay, and its state.
    active: Option<usize>,
    state: InteractionState,
    on_press: Option<ErasedArgCallback<usize>>,
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::card`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved state-layer content color. Themed: `colors.on_surface`.
/// Unthemed: [`ON_SURFACE`] exactly.
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

/// The resolved fill for one card. `disabled` overrides everything with the
/// dimmed `on_surface` wash [`super::card`] uses; a selected index takes
/// `colors.secondary_container`; an explicit `override_color` replaces the
/// filled-card role `colors.surface_container_highest`.
fn resolve_card_color(
    theme: Option<&Theme>,
    override_color: Option<Color>,
    selected: bool,
    disabled: bool,
) -> Color {
    if disabled {
        return with_alpha(resolve_content_color(theme), DISABLED_CONTAINER_OPACITY);
    }
    if selected {
        return match theme {
            Some(theme) => theme.scheme().secondary_container,
            None => SELECTED_CONTAINER,
        };
    }
    if let Some(color) = override_color {
        return color;
    }
    match theme {
        Some(theme) => theme.scheme().surface_container_highest,
        None => FILLED_CONTAINER,
    }
}

impl<State: 'static> View<State> for CardListView<State> {
    type Element = CardListWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CardListWidget {
        let items = self
            .items
            .iter()
            .map(|view| frust::authoring::build_child(view, ctx))
            .collect();
        CardListWidget {
            items,
            outer_radius: self.outer_radius,
            inner_radius: self.inner_radius,
            gap: self.gap,
            item_padding: self.item_padding,
            color: self.color,
            selected: self.selected.clone(),
            interactive: self.on_press.is_some(),
            enabled: self.enabled,
            haptic: self.haptic,
            cards: Vec::new(),
            width: 0.0,
            armed: None,
            active: None,
            state: InteractionState::new(),
            on_press: self
                .on_press
                .as_ref()
                .map(frust::authoring::erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CardListWidget,
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
            // index may now name a different card (or none at all).
            element.armed = None;
            element.active = None;
            element.state.set_pressed(false);
            element.state.set_hovered(false);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        if element.outer_radius != self.outer_radius
            || element.inner_radius != self.inner_radius
            || element.color != self.color
            || element.selected != self.selected
        {
            element.outer_radius = self.outer_radius;
            element.inner_radius = self.inner_radius;
            element.color = self.color;
            element.selected = self.selected.clone();
            flags |= ChangeFlags::PAINT;
        }
        if element.gap != self.gap || element.item_padding != self.item_padding {
            element.gap = self.gap;
            element.item_padding = self.item_padding;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            if !now_interactive {
                // Losing the interactive surface mid-gesture must not leave a
                // dangling capture, press, or hover behind.
                element.armed = None;
                element.active = None;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled {
                element.armed = None;
                element.active = None;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        element.haptic = self.haptic;
        element.on_press = self
            .on_press
            .as_ref()
            .map(frust::authoring::erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut CardListWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.items.iter().zip(element.items.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl CardListWidget {
    /// The local-space rect of card `index`, or `None` before the first
    /// layout pass (or past the end).
    fn card_rect(&self, index: usize) -> Option<(Point, Size)> {
        let (y, height) = *self.cards.get(index)?;
        Some((Point::new(0.0, y), Size::new(self.width, height)))
    }

    /// The card containing local `pos`, if any — a position in a gap between
    /// two cards belongs to neither.
    fn card_at(&self, pos: Point) -> Option<usize> {
        if pos.x < 0.0 || pos.x >= self.width {
            return None;
        }
        self.cards
            .iter()
            .position(|(y, height)| pos.y >= *y && pos.y < y + height)
    }

    /// The radii card `index` of this stack takes.
    fn radii_at(&self, index: usize) -> CornerRadii {
        card_radii(
            card_position(index, self.items.len()),
            self.outer_radius,
            self.inner_radius,
        )
    }

    /// Clear whatever gesture state a press left behind (shared by the
    /// `Up`/`Cancel` arms and by the rebuild paths above).
    fn clear_press(&mut self) {
        self.armed = None;
        self.state.set_pressed(false);
        if !self.state.is_active() {
            self.active = None;
        }
    }
}

impl Widget for CardListWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        self.width = width;
        let inner_width = (width - self.item_padding * 2.0).max(0.0);
        let max_height = bc.max().height;
        // Tight width, loose height: each card spans the stack
        // (`M3ECard(width: double.infinity)`) and takes its child's height.
        let child_bc = BoxConstraints::new(
            Size::new(inner_width, 0.0),
            Size::new(inner_width, max_height),
        );

        self.cards.clear();
        let mut y = 0.0;
        let count = self.items.len();
        for (index, pod) in self.items.iter_mut().enumerate() {
            let child = pod.layout_child(ctx, &child_bc);
            let card_height = child.height + self.item_padding * 2.0;
            pod.set_origin(Point::new(self.item_padding, y + self.item_padding));
            self.cards.push((y, card_height));
            y += card_height;
            if index + 1 < count {
                y += self.gap;
            }
        }

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens before `ctx` is taken mutably below
        // (`paint_child`) — mirrors `card::CardWidget::paint`'s ordering.
        let theme = Theme::from_paint_ctx(ctx);
        let disabled = self.interactive && !self.enabled;
        let content_color = resolve_content_color(theme);
        let fills: Vec<Color> = (0..self.items.len())
            .map(|index| {
                resolve_card_color(theme, self.color, self.selected.contains(&index), disabled)
            })
            .collect();
        // Authoritative hover read, self-correcting the latched flag — inert
        // while non-interactive or disabled (`docs/CODE_STANDARDS.md`'s
        // Interaction Semantics; never react to hover while disabled).
        let hovered = self.interactive && self.enabled && ctx.is_hovered();
        self.state.set_hovered(hovered);
        if !hovered && !self.state.pressed {
            self.active = None;
        }

        let origin = ctx.origin();
        let overlay_opacity = if self.interactive && self.enabled {
            self.state.resolve_opacity()
        } else {
            0.0
        };

        for (index, fill) in fills.iter().enumerate() {
            let Some((card_origin, size)) = self.card_rect(index) else {
                continue;
            };
            let absolute = Point::new(origin.x + card_origin.x, origin.y + card_origin.y);
            let radii = self.radii_at(index);
            scene.fill_rounded_rect_radii(absolute, size, radii, *fill);
            if overlay_opacity > 0.0 && self.active == Some(index) {
                scene.fill_rounded_rect_radii(
                    absolute,
                    size,
                    radii,
                    with_alpha(content_color, overlay_opacity),
                );
            }
        }

        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return frust::authoring::route_event(&mut self.items, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to the
        // children, even when interactive. Only pointer events drive the
        // interactive stack's own capture/press/hover behavior.
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.items, ctx, event);
        };
        if !self.enabled {
            // A disabled interactive stack arms nothing and claims no hover,
            // and does not forward to its children either — mirrors `card`'s
            // disabled early-return.
            return EventResult::Ignored;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.card_at(p.position) else {
                    // The gap between two cards is nobody's target.
                    return EventResult::Ignored;
                };
                self.armed = Some(index);
                self.active = Some(index);
                self.state.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let Some(armed) = self.armed else {
                    // No capture: this is the hover pass — claim, latch, and
                    // let paint self-correct (`docs/CODE_STANDARDS.md`'s
                    // three-part hover contract).
                    let over = self.card_at(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                    }
                    let changed = self.state.set_hovered(over.is_some()) || self.active != over;
                    self.active = over;
                    if changed {
                        ctx.request_redraw();
                    }
                    return EventResult::Ignored;
                };
                let still_on_card = self.card_at(p.position) == Some(armed);
                if self.state.set_pressed(still_on_card) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed else {
                    return EventResult::Ignored;
                };
                if self.card_at(p.position) == Some(armed) {
                    if self.haptic != HapticSignal::None {
                        MaterialHaptics::fire(self.haptic);
                    }
                    if let Some(on_press) = self.on_press.as_mut() {
                        (on_press)(ctx, armed);
                    }
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
            |node| {
                if self.interactive {
                    if self.enabled {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                }
            },
            |ctx| {
                for pod in &self.items {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_widgets::test_support::leaf_any;
    use std::any::Any;

    const WIDTH: f64 = 300.0;
    /// Each test child is this tall, so a card is `ITEM + 2 * padding`.
    const CHILD_HEIGHT: f64 = 20.0;

    fn card_height() -> f64 {
        CHILD_HEIGHT + CARD_LIST_ITEM_PADDING * 2.0
    }

    fn build<S: 'static>(view: &CardListView<S>) -> CardListWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CardListWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(WIDTH, 1000.0)))
    }

    fn leaves(count: usize) -> Vec<AnyView<()>> {
        (0..count).map(|_| leaf_any(10.0, CHILD_HEIGHT)).collect()
    }

    /// Records every per-corner rounded rect's `(origin, size, radii, color)`.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, CornerRadii, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, color: Color) {
            self.rrects.push((o, s, radii, color));
        }
    }

    impl Recorder {
        /// The opaque card fills (the translucent state-layer overlays are
        /// told apart by alpha).
        fn fills(&self) -> Vec<(Point, CornerRadii, Color)> {
            self.rrects
                .iter()
                .filter(|(_, _, _, c)| c.components[3] >= 1.0)
                .map(|(o, _, radii, c)| (*o, *radii, *c))
                .collect()
        }

        /// The translucent state-layer overlays painted this frame.
        fn overlays(&self) -> Vec<(Point, CornerRadii, f32)> {
            self.rrects
                .iter()
                .filter(|(_, _, _, c)| c.components[3] < 1.0)
                .map(|(o, _, radii, c)| (*o, *radii, c.components[3]))
                .collect()
        }
    }

    fn paint(w: &mut CardListWidget, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let size = Size::new(WIDTH, 1000.0);
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    // --- The corner matrix ---

    #[test]
    fn card_position_classifies_single_first_middle_last() {
        assert_eq!(card_position(0, 1), CardPosition::Single);
        assert_eq!(card_position(0, 3), CardPosition::First);
        assert_eq!(card_position(1, 3), CardPosition::Middle);
        assert_eq!(card_position(2, 3), CardPosition::Last);
        // Degenerate inputs resolve rather than panic.
        assert_eq!(card_position(0, 0), CardPosition::Single);
        assert_eq!(card_position(9, 3), CardPosition::Last);
    }

    #[test]
    fn card_radii_pins_the_reference_corner_matrix() {
        let (outer, inner) = (CARD_LIST_OUTER_RADIUS, CARD_LIST_INNER_RADIUS);
        assert_eq!(
            card_radii(CardPosition::Single, outer, inner),
            CornerRadii::uniform(outer)
        );
        assert_eq!(
            card_radii(CardPosition::First, outer, inner),
            CornerRadii::new(outer, outer, inner, inner),
            "first: outer on top, inner on the joined bottom edge"
        );
        assert_eq!(
            card_radii(CardPosition::Middle, outer, inner),
            CornerRadii::uniform(inner)
        );
        assert_eq!(
            card_radii(CardPosition::Last, outer, inner),
            CornerRadii::new(inner, inner, outer, outer),
            "last: inner on the joined top edge, outer on the bottom"
        );
    }

    #[test]
    fn a_painted_stack_carries_the_position_radii_in_order() {
        let view: CardListView<()> = card_list(leaves(3));
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, None);
        let (outer, inner) = (CARD_LIST_OUTER_RADIUS, CARD_LIST_INNER_RADIUS);
        let radii: Vec<CornerRadii> = rec.fills().iter().map(|(_, r, _)| *r).collect();
        assert_eq!(
            radii,
            vec![
                card_radii(CardPosition::First, outer, inner),
                card_radii(CardPosition::Middle, outer, inner),
                card_radii(CardPosition::Last, outer, inner),
            ]
        );
    }

    #[test]
    fn a_lone_card_takes_the_single_radii() {
        let view: CardListView<()> = card_list(leaves(1));
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, None);
        let fills = rec.fills();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].1, CornerRadii::uniform(CARD_LIST_OUTER_RADIUS));
    }

    #[test]
    fn overridden_radii_reach_the_painted_cards() {
        let view: CardListView<()> = card_list(leaves(2)).outer_radius(30.0).inner_radius(2.0);
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, None);
        let radii: Vec<CornerRadii> = rec.fills().iter().map(|(_, r, _)| *r).collect();
        assert_eq!(
            radii,
            vec![
                CornerRadii::new(30.0, 30.0, 2.0, 2.0),
                CornerRadii::new(2.0, 2.0, 30.0, 30.0),
            ]
        );
    }

    // --- Layout: padding, gap, stacking ---

    #[test]
    fn cards_stack_with_the_gap_and_pad_their_children() {
        let view: CardListView<()> = card_list(leaves(3));
        let mut w = build(&view);
        let size = layout(&mut w);
        let card = card_height();
        assert_eq!(size.width, WIDTH);
        assert_eq!(size.height, card * 3.0 + CARD_LIST_GAP * 2.0);

        // Each child is inset by the item padding inside its own card.
        for (index, pod) in w.items.iter().enumerate() {
            let card_top = (card + CARD_LIST_GAP) * index as f64;
            assert_eq!(
                pod.origin(),
                Point::new(CARD_LIST_ITEM_PADDING, card_top + CARD_LIST_ITEM_PADDING)
            );
        }
        // The last card contributes no trailing gap (`isLast ? 0 : gap`).
        let (last_y, last_h) = w.cards[2];
        assert_eq!(last_y + last_h, size.height);
    }

    #[test]
    fn an_empty_stack_lays_out_and_paints_nothing() {
        let view: CardListView<()> = card_list(Vec::new());
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size.height, 0.0);
        assert!(paint(&mut w, None).rrects.is_empty());
    }

    // --- Fills: default, selected, override, disabled ---

    #[test]
    fn cards_take_the_filled_role_and_selected_indices_the_selection_fill() {
        let theme = crate::baseline();
        let view: CardListView<()> = card_list(leaves(3)).selected([1]);
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, Some(&theme));
        let colors: Vec<Color> = rec.fills().iter().map(|(_, _, c)| *c).collect();
        let scheme = theme.scheme();
        assert_eq!(
            colors,
            vec![
                scheme.surface_container_highest,
                scheme.secondary_container,
                scheme.surface_container_highest,
            ]
        );
    }

    #[test]
    fn an_explicit_color_replaces_the_unselected_fill_only() {
        let theme = crate::baseline();
        let custom = Color::from_rgb8(0x10, 0x20, 0x30);
        let view: CardListView<()> = card_list(leaves(2)).color(custom).selected([1]);
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, Some(&theme));
        let colors: Vec<Color> = rec.fills().iter().map(|(_, _, c)| *c).collect();
        assert_eq!(colors, vec![custom, theme.scheme().secondary_container]);
    }

    #[test]
    fn a_disabled_interactive_stack_dims_every_card_and_paints_no_overlay() {
        let theme = crate::baseline();
        let view: CardListView<()> = card_list(leaves(2))
            .on_press(|_: &mut (), _| {})
            .selected([0])
            .enabled(false);
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, Some(&theme));
        let dimmed = with_alpha(theme.scheme().on_surface, DISABLED_CONTAINER_OPACITY);
        // The dim wash is translucent, so it lands in `overlays()` by alpha —
        // assert against the raw rects instead.
        assert_eq!(rec.rrects.len(), 2);
        for (_, _, _, color) in &rec.rrects {
            assert_eq!(*color, dimmed, "every card dims, selected included");
        }
    }

    // --- Interaction ---

    #[derive(Default)]
    struct Pressed {
        indices: Vec<usize>,
    }

    fn dispatch<S: 'static>(
        w: &mut CardListWidget,
        state: &mut S,
        phase: PointerPhase,
        x: f64,
        y: f64,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(WIDTH, 1000.0));
        w.event(
            &mut ctx,
            &InputEvent::Pointer(frust::authoring::PointerEvent {
                phase,
                position: Point::new(x, y),
                button: frust::authoring::PointerButton::Primary,
            }),
        )
    }

    fn interactive_stack() -> CardListWidget {
        let view: CardListView<Pressed> = card_list(
            (0..3).map(|_| any::<Pressed, _>(frust::SizedBox(Some(10.0), Some(CHILD_HEIGHT)))),
        )
        .on_press(|s: &mut Pressed, index| s.indices.push(index));
        let mut w = build(&view);
        layout(&mut w);
        w
    }

    /// The y-centre of card `index`.
    fn card_y(index: usize) -> f64 {
        (card_height() + CARD_LIST_GAP) * index as f64 + card_height() / 2.0
    }

    #[test]
    fn an_interactive_stack_fires_the_released_cards_index() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, card_y(1));
        assert_eq!(w.armed, Some(1));
        dispatch(&mut w, &mut state, PointerPhase::Up, 50.0, card_y(1));
        assert_eq!(state.indices, vec![1]);
        assert_eq!(w.armed, None);
    }

    #[test]
    fn a_release_over_another_card_fires_nothing() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, card_y(0));
        dispatch(&mut w, &mut state, PointerPhase::Move, 50.0, card_y(2));
        dispatch(&mut w, &mut state, PointerPhase::Up, 50.0, card_y(2));
        assert!(state.indices.is_empty());
    }

    #[test]
    fn a_press_in_the_gap_between_cards_is_ignored() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        // Half a gap past the first card's bottom edge.
        let gap_y = card_height() + CARD_LIST_GAP / 2.0;
        assert_eq!(
            dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, gap_y),
            EventResult::Ignored
        );
        assert_eq!(w.armed, None);
    }

    #[test]
    fn a_secondary_press_never_arms_or_fires() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(WIDTH, 1000.0));
        let result = w.event(
            &mut ctx,
            &InputEvent::Pointer(frust::authoring::PointerEvent {
                phase: PointerPhase::Down,
                position: Point::new(50.0, card_y(0)),
                button: frust::authoring::PointerButton::Secondary,
            }),
        );
        assert_eq!(result, EventResult::Ignored);
        assert_eq!(w.armed, None);
    }

    #[test]
    fn a_cancel_clears_the_armed_card_without_firing() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, card_y(0));
        dispatch(&mut w, &mut state, PointerPhase::Cancel, 50.0, card_y(0));
        assert_eq!(w.armed, None);
        dispatch(&mut w, &mut state, PointerPhase::Up, 50.0, card_y(0));
        assert!(state.indices.is_empty());
    }

    #[test]
    fn a_disabled_stack_ignores_every_pointer_phase() {
        let view: CardListView<Pressed> = card_list(
            (0..2).map(|_| any::<Pressed, _>(frust::SizedBox(Some(10.0), Some(CHILD_HEIGHT)))),
        )
        .on_press(|s: &mut Pressed, index| s.indices.push(index))
        .enabled(false);
        let mut w = build(&view);
        layout(&mut w);
        let mut state = Pressed::default();
        assert_eq!(
            dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, card_y(0)),
            EventResult::Ignored
        );
        dispatch(&mut w, &mut state, PointerPhase::Up, 50.0, card_y(0));
        assert!(state.indices.is_empty());
        assert_eq!(w.armed, None);
    }

    #[test]
    fn the_pressed_card_paints_its_overlay_at_its_own_radii() {
        let mut w = interactive_stack();
        let mut state = Pressed::default();
        dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, card_y(2));
        let rec = paint(&mut w, None);
        let overlays = rec.overlays();
        assert_eq!(overlays.len(), 1, "only the pressed card tints");
        assert_eq!(overlays[0].0.y, w.cards[2].0);
        assert_eq!(
            overlays[0].1,
            card_radii(
                CardPosition::Last,
                CARD_LIST_OUTER_RADIUS,
                CARD_LIST_INNER_RADIUS
            )
        );
        assert_eq!(overlays[0].2, frust::authoring::PRESSED_OPACITY);
    }

    #[test]
    fn a_non_interactive_stack_routes_events_into_its_children() {
        // Without a list-level `on_press`, a hosted row's own press handling
        // is what runs — the stack is a plain container.
        #[derive(Default)]
        struct Counter {
            presses: u32,
        }
        let view: CardListView<Counter> = card_list_items(vec![
            crate::list_item::list_item::<Counter>("row")
                .on_press(|s: &mut Counter| s.presses += 1),
        ]);
        let mut w = build(&view);
        layout(&mut w);
        let mut state = Counter::default();
        let row_y = CARD_LIST_ITEM_PADDING + 5.0;
        assert_eq!(
            dispatch(&mut w, &mut state, PointerPhase::Down, 50.0, row_y),
            EventResult::Handled,
            "the row itself takes the press"
        );
        dispatch(&mut w, &mut state, PointerPhase::Up, 50.0, row_y);
        assert_eq!(state.presses, 1);
        assert_eq!(w.armed, None, "the stack armed nothing of its own");
    }

    #[test]
    fn card_list_items_hosts_rows_that_paint_no_surface_of_their_own() {
        // A hosted row paints no container of its own: only the stack's own
        // per-card fills reach the scene.
        let view: CardListView<()> = card_list_items(vec![
            crate::list_item::list_item::<()>("a"),
            crate::list_item::list_item::<()>("b"),
        ]);
        let mut w = build(&view);
        layout(&mut w);
        let rec = paint(&mut w, None);
        assert_eq!(
            rec.rrects.len(),
            2,
            "two card fills, and no row-owned surface underneath them"
        );
    }

    // --- Semantics ---

    #[test]
    fn semantics_is_a_list_container_forwarding_its_rows() {
        fn logic(_s: &mut ()) -> CardListView<()> {
            card_list_items(vec![
                crate::list_item::list_item::<()>("a"),
                crate::list_item::list_item::<()>("b"),
            ])
        }
        let mut root: frust_core::RenderRoot<(), CardListView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(WIDTH, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("the stack contributes a Role::List node");
        assert_eq!(list.children().len(), 2, "both rows are semantics children");
        assert_eq!(
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == Role::ListItem)
                .count(),
            2
        );
    }

    #[test]
    fn semantics_an_interactive_stack_carries_click_and_a_disabled_one_reports_disabled() {
        fn interactive(_s: &mut ()) -> CardListView<()> {
            card_list(vec![leaf_any(10.0, CHILD_HEIGHT)]).on_press(|_: &mut (), _| {})
        }
        let mut root: frust_core::RenderRoot<(), CardListView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut interactive, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(WIDTH, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("list node");
        assert!(list.supports_action(Action::Click));
        assert!(!list.is_disabled());

        fn disabled(_s: &mut ()) -> CardListView<()> {
            card_list(vec![leaf_any(10.0, CHILD_HEIGHT)])
                .on_press(|_: &mut (), _| {})
                .enabled(false)
        }
        let mut root: frust_core::RenderRoot<(), CardListView<()>> = frust_core::RenderRoot::new();
        root.rebuild(&mut disabled, &mut state);
        root.layout_with_text(Size::new(WIDTH, 400.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, list) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::List)
            .expect("list node");
        assert!(list.is_disabled());
        assert!(!list.supports_action(Action::Click));
    }

    // --- Hover, through a real RenderRoot ---
    //
    // Hover is an authoritative read off `PaintCtx`, seeded by the pod chain,
    // so only a real tree can exercise it (the harness shape
    // `list_item`/`card` both use).

    struct HoverHarness {
        root: frust_core::RenderRoot<Pressed, CardListView<Pressed>>,
        state: Pressed,
        tcx: TextContext,
        enabled: bool,
    }

    impl HoverHarness {
        fn new(enabled: bool) -> Self {
            let mut h = HoverHarness {
                root: frust_core::RenderRoot::new(),
                state: Pressed::default(),
                tcx: TextContext::new(),
                enabled,
            };
            let enabled_flag = enabled;
            let mut app =
                move |_s: &mut Pressed| {
                    card_list((0..3).map(|_| {
                        any::<Pressed, _>(frust::SizedBox(Some(10.0), Some(CHILD_HEIGHT)))
                    }))
                    .on_press(|s: &mut Pressed, index| s.indices.push(index))
                    .enabled(enabled_flag)
                };
            h.root.rebuild(&mut app, &mut h.state);
            h.root
                .layout_with_text(Size::new(WIDTH, 400.0), &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, phase: PointerPhase, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust::authoring::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust::authoring::PointerButton::Primary,
                }),
            );
        }

        /// The overlays painted this frame, as `(card index, alpha)`.
        fn overlays(&mut self) -> Vec<(usize, f32)> {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.overlays()
                .iter()
                .map(|(origin, _, alpha)| {
                    let index = (origin.y / (card_height() + CARD_LIST_GAP)).round() as usize;
                    (index, *alpha)
                })
                .collect()
        }

        /// Every rect painted this frame — the disabled wash is itself
        /// translucent, so a disabled stack is asserted by rect *count*
        /// rather than through [`Self::overlays`]'s alpha filter.
        fn rect_count(&mut self) -> usize {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.rrects.len()
        }
    }

    #[test]
    fn hovering_a_card_tints_that_card_only() {
        let mut h = HoverHarness::new(true);
        assert!(h.overlays().is_empty(), "no overlay at rest");

        h.dispatch(PointerPhase::Move, 50.0, card_y(1));
        assert_eq!(
            h.overlays(),
            vec![(1, crate::interaction::HOVER_OPACITY)],
            "the hovered card tints at the M3E hover opacity"
        );

        h.dispatch(PointerPhase::Move, 50.0, card_y(2));
        assert_eq!(h.overlays(), vec![(2, crate::interaction::HOVER_OPACITY)]);

        // Off every card (past the stack), nothing tints.
        h.dispatch(PointerPhase::Move, 50.0, 390.0);
        assert!(h.overlays().is_empty());
    }

    #[test]
    fn a_disabled_stack_never_tints_and_never_fires() {
        let mut h = HoverHarness::new(false);
        h.dispatch(PointerPhase::Move, 50.0, card_y(0));
        assert_eq!(
            h.rect_count(),
            3,
            "three dimmed card fills and no state layer: a disabled stack claims no hover"
        );
        h.dispatch(PointerPhase::Down, 50.0, card_y(0));
        h.dispatch(PointerPhase::Up, 50.0, card_y(0));
        assert!(h.state.indices.is_empty());
        assert!(!h.enabled);
    }
}
