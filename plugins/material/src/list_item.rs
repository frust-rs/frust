// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments)
// Upstream: https://github.com/paadevelopments/material_3_expressive
//   lib/components/lists/ (`m3e_lists.dart`'s `M3EListItem`,
//   `styles/m3e_list_theme.dart`'s `M3EListItemTheme`,
//   `components/m3e_list_item_scope.dart`)
// This rework adds the reference's overline slot, selected tint, and its
// card-backed container surface (opt-in here rather than default — see the
// module docs' Container surface section) on top of the v1 transparent
// fixed-height row; the reference's `variant`/`border` customization escape
// hatches stay unported — see the module docs' Not ported section.

//! `ListItem` rows: 1/2/3-line variants at 56/72/88dp heights, meant to pair
//! with [`frust::ListView`] or [`mod@super::card_list`].
//!
//! # Anatomy
//!
//! A row is `[ leading? | (overline? / headline / supporting?) | trailing? ]`
//! with 16dp horizontal padding (`M3EListItemTheme.horizontalPadding`) and a
//! 16dp gap (`.gap`) between the slots and the text column. The
//! `overline`/`headline`/`supporting` runs are child [`frust::text`] widgets
//! laid out during the layout pass — [`frust::authoring::PaintCtx`] has no
//! text-shaping context, so all text sizing happens at layout time — carrying
//! the reference's own type/role/truncation mapping:
//!
//! | Run | Type token | Color role | Lines |
//! |---|---|---|---|
//! | `overline` | label-small (11sp, 0.5 tracking) | `on_surface_variant` | 1, ellipsized |
//! | `headline` | body-large (16sp, the default) | `on_surface` | 1, ellipsized |
//! | `supporting` | body-medium (14sp) | `on_surface_variant` | 2, ellipsized |
//!
//! The `leading`/`trailing` slots are arbitrary [`frust::authoring::AnyView`]s
//! — an icon, avatar, image, checkbox, switch, or a trailing supporting-text
//! run — exactly as upstream types them (`Widget? leading` / `Widget?
//! trailing`); there is no closed slot vocabulary to port. They are vertically
//! centered, except on a three-line row, where they pin to the top inset
//! (`crossAxisAlignment: threeLine ? start : center`).
//!
//! # Height
//!
//! | Variant | Height | Constructed by |
//! |---|---|---|
//! | One-line   | 56dp | [`list_item`] |
//! | Two-line   | 72dp | [`ListItem::supporting`] / [`ListItem::overline`] |
//! | Three-line | 88dp | overline **and** supporting, or [`ListItem::three_line`] |
//!
//! Fixed heights are this port's own model (M3's list spec) rather than the
//! reference's intrinsic `minHeight: 40` + 8/12dp vertical padding — one-line
//! rows agree exactly (40 + 2×8 = 56), and a fixed extent is what
//! [`frust::list_view`]'s uniform-extent fast path needs. The three-line
//! promotion rule *is* the reference's (`_isThreeLine => supportingText !=
//! null && overline != null`), and [`ListItem::three_line`] still forces it.
//!
//! # Container surface
//!
//! [`ListItem::contained`] paints the reference's own row surface —
//! `M3EListItem` wraps its body in an `M3ECard(variant: filled)`, so a
//! contained row fills `surface_container_highest` at the `shape.medium`
//! (12dp) radius, and `secondary_container` while [`ListItem::selected`]
//! (`M3EListItemTheme.selectedColor`).
//!
//! **It is opt-in here, where upstream has it on by default** — a documented
//! deviation, for two reasons. First, the reference's own default is
//! conditional in practice: `M3EListItemScope` makes every row hosted by a
//! card-backed list (`M3ECardList`, the dismissible/expandable lists) drop the
//! card and render its body only, because the host already owns the surface —
//! and [`mod@super::card_list`] is where that shape lives in this catalog.
//! Second, this catalog's rows predate the M3E surface and are placed by
//! consumers inside their own scrollers and containers, where an
//! unconditional per-row card would stack surfaces (and scallop adjacent rows'
//! 12dp corners) rather than group them.
//!
//! So a bare row is content — the port of what the reference's embedded rows
//! render — and a row that wants the standalone card asks for it. A
//! **selected** row still paints its selection fill either way: `selected` is
//! a state the user must see, not a container preference.
//!
//! # Interactivity
//!
//! [`ListItem::on_press`] makes the whole row one interactive target,
//! mirroring [`mod@super::card`]: the row owns capture, fires on release
//! inside its bounds, and paints an [`crate::interaction::InteractionState`]
//! overlay tinted `on_surface`. The overlay takes the container's radius when
//! the row paints one, and square corners when it does not — the host owns
//! whatever shape is under a bare row, and a 12dp overlay corner inside a
//! differently-rounded host pokes out at the corners. A non-interactive row
//! routes pointer events to its slot children (so a trailing control stays
//! live).
//!
//! An interactive row is also the catalog's **hover** reference consumer, and
//! shows the whole three-part contract: it claims the hover link from its
//! uncaptured `Move` arm ([`frust::authoring::EventCtx::claim_hover`]), latches the
//! same hit test into [`crate::interaction::InteractionState::set_hovered`] and requests
//! a redraw only when that flag changes (the frame that makes the 8% overlay appear
//! on entry), and re-syncs the flag from
//! [`frust::authoring::PaintCtx::is_hovered`] every paint — authoritative, because
//! a pointer *leaving* the row routes its next move to whatever it moved onto, so
//! the row never hears about the departure. A press wins visually while it lasts
//! (pressed 10% > hover 8%, the M3E precedence order), and a **captured** drag
//! paints no hover at all, the framework refusing a claim from a captured pointer.
//! A touch drag that captured nothing is an ordinary hover pass, though, so the row
//! can tint under a finger until the lift's `Up` ends the link.
//!
//! # Disabled state
//!
//! [`ListItem::enabled`] (default `true`) is this crate's own extension — the
//! reference `M3EListItem` carries no `enabled` field — following the same
//! convention [`mod@super::card`] established: an interactive row that is
//! disabled keeps its [`Role::ListItem`] semantics (reporting
//! `Node::set_disabled()` instead of [`Action::Click`]) and dims its container
//! to `on_surface` at [`crate::interaction::DISABLED_CONTAINER_OPACITY`], but
//! claims no hover, paints no state layer, and fires nothing. **Hover is
//! enabled-gated** in both the event pass and paint's self-correction (never
//! react to hover while disabled). A disabled interactive row's `Widget::event`
//! early-returns `Ignored` for every pointer phase rather than forwarding to
//! its slots — a disabled row is fully inert, not a pass-through (mirroring
//! `card`'s identical disabled early-return). `enabled` is a no-op on a
//! non-interactive row, which has no interactive surface to gate.
//!
//! Text runs are *not* dimmed: their color resolves inside
//! [`frust::text`]'s own themed-role path at layout time, which this row
//! cannot override without baking an explicit unthemed color. The container
//! wash is the whole disabled treatment here, so a disabled row with no
//! container painted (neither `contained` nor `selected`) is gated but not
//! dimmed.
//!
//! # Not ported
//!
//! The reference's `variant`/`border` card overrides (and the
//! `M3EListItemTheme` knobs behind them) stay unported, the same scope line
//! [`mod@super::card`] draws around its own customization escape hatches: a
//! [`ListItem::contained`] row is the filled variant, and a row that needs
//! another surface goes inside the container that paints it.
//! `IconTheme.merge`'s implicit leading/trailing icon sizing/tinting has no
//! frust analogue either — a slot view carries its own size and color.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::text::TextOverflow;
use frust::authoring::{Action, Role};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx,
    ThemeTextColor, ThemeTextType, View, Widget, any,
};
use frust::text;
use kurbo::{Point, Size};
use peniko::Color;

use crate::interaction::{DISABLED_CONTAINER_OPACITY, InteractionState};

use super::press::presses;

/// Horizontal padding on the leading and trailing edges
/// (`M3EListItemTheme.horizontalPadding`, 16dp).
const HPAD: f64 = 16.0;
/// Gap between a leading/trailing slot and the text column
/// (`M3EListItemTheme.gap`, 16dp).
const GAP: f64 = 16.0;
/// Vertical inset a three-line row's slots pin to
/// (`M3EListItemTheme.threeLineVerticalPadding`, 12dp) — the reference's
/// `crossAxisAlignment: start` case.
const VPAD_THREE_LINE: f64 = 12.0;
/// Supporting-text font size (M3 body-medium, 14sp); the headline keeps the
/// default 16sp body-large size.
const SUPPORTING_SIZE: f32 = 14.0;
/// Overline font size (M3 label-small, 11sp).
const OVERLINE_SIZE: f32 = 11.0;
/// Overline letter spacing (M3 label-small, 0.5).
const OVERLINE_TRACKING: f32 = 0.5;
/// Rendered line cap for the headline (`maxLines: 1`, ellipsized).
const HEADLINE_MAX_LINES: usize = 1;
/// Rendered line cap for the supporting run (`maxLines: 2`, ellipsized).
const SUPPORTING_MAX_LINES: usize = 2;

/// One-line row height, in logical px (M3 list spec).
pub const ONE_LINE_HEIGHT: f64 = 56.0;
/// Two-line row height, in logical px (M3 list spec).
pub const TWO_LINE_HEIGHT: f64 = 72.0;
/// Three-line row height, in logical px (M3 list spec).
pub const THREE_LINE_HEIGHT: f64 = 88.0;

/// Corner radius of a standalone row's container (unthemed fallback; a theme
/// resolves this from `shape.medium`, the 12dp token `M3ECardTheme`'s
/// `radiusMedium` names).
const RADIUS: f64 = 12.0;

/// Unthemed-fallback state-layer content color for an interactive row (a theme
/// resolves this from `colors.on_surface`).
const ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback container fill for a standalone row (a theme resolves
/// this from `colors.surface_container_highest`, the filled-card role).
const FILLED_CONTAINER: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback container fill for a selected row (a theme resolves this
/// from `colors.secondary_container`, `M3EListItemTheme.selectedColor`).
const SELECTED_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);

/// The number of text lines a [`ListItem`] reserves height for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListItemLines {
    /// Headline only — 56dp.
    One,
    /// Headline + one more run (supporting or overline) — 72dp.
    Two,
    /// Overline + headline + supporting — 88dp.
    Three,
}

impl ListItemLines {
    /// The fixed row height for this line count.
    pub fn height(self) -> f64 {
        match self {
            ListItemLines::One => ONE_LINE_HEIGHT,
            ListItemLines::Two => TWO_LINE_HEIGHT,
            ListItemLines::Three => THREE_LINE_HEIGHT,
        }
    }
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// A declarative M3 list row. See the [module docs](self).
pub struct ListItem<State: 'static> {
    headline: String,
    supporting: Option<String>,
    overline: Option<String>,
    lines: ListItemLines,
    leading: Option<AnyView<State>>,
    trailing: Option<AnyView<State>>,
    on_press: Option<OnPress<State>>,
    selected: bool,
    enabled: bool,
    contained: bool,
}

/// Create a one-line list row with the given `headline` text.
pub fn list_item<State: 'static>(headline: impl Into<String>) -> ListItem<State> {
    ListItem {
        headline: headline.into(),
        supporting: None,
        overline: None,
        lines: ListItemLines::One,
        leading: None,
        trailing: None,
        on_press: None,
        selected: false,
        enabled: true,
        contained: false,
    }
}

impl<State: 'static> ListItem<State> {
    /// Add supporting text below the headline, promoting the row's height per
    /// the reference's line rule (see the [module docs](self)' Height table).
    pub fn supporting(mut self, supporting: impl Into<String>) -> Self {
        self.supporting = Some(supporting.into());
        self.promote_lines();
        self
    }

    /// Add an overline label above the headline (`M3EListItem.overline`),
    /// promoting the row's height per the reference's line rule (see the
    /// [module docs](self)' Height table).
    pub fn overline(mut self, overline: impl Into<String>) -> Self {
        self.overline = Some(overline.into());
        self.promote_lines();
        self
    }

    /// Raise the reserved height to whatever the present runs imply —
    /// overline **and** supporting is the reference's three-line case
    /// (`_isThreeLine`), either one alone is two-line. Never *lowers* the
    /// count, so an explicit [`ListItem::three_line`] survives a later
    /// `supporting`/`overline` call regardless of ordering.
    fn promote_lines(&mut self) {
        let implied = match (self.overline.is_some(), self.supporting.is_some()) {
            (true, true) => ListItemLines::Three,
            (true, false) | (false, true) => ListItemLines::Two,
            (false, false) => ListItemLines::One,
        };
        if implied.height() > self.lines.height() {
            self.lines = implied;
        }
    }

    /// Force the three-line (88dp) variant (for a supporting line that wraps to
    /// two visual lines).
    pub fn three_line(mut self) -> Self {
        self.lines = ListItemLines::Three;
        self
    }

    /// Set the leading slot (an icon/avatar/image/control) — vertically
    /// centered, or pinned to the top inset on a three-line row.
    pub fn leading<V: View<State>>(mut self, leading: V) -> Self {
        self.leading = Some(any(leading));
        self
    }

    /// Set the trailing slot (an icon, a metadata/supporting-text run, a
    /// checkbox/switch) — vertically centered, or pinned to the top inset on a
    /// three-line row.
    pub fn trailing<V: View<State>>(mut self, trailing: V) -> Self {
        self.trailing = Some(any(trailing));
        self
    }

    /// Make the whole row interactive, firing `on_press` on release inside its
    /// bounds (see the module docs' Interactivity section).
    pub fn on_press<F: Fn(&mut State) + 'static>(mut self, on_press: F) -> Self {
        self.on_press = Some(Rc::new(on_press));
        self
    }

    /// Paint the row's container with the selected fill
    /// (`colors.secondary_container`, `M3EListItemTheme.selectedColor`) and
    /// report the state to accessibility. Defaults to `false`. A selected row
    /// paints its fill whether or not it is [`ListItem::contained`] — the
    /// state has to be visible.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Paint the reference's standalone row surface: the filled-card
    /// container (`colors.surface_container_highest`) at the `shape.medium`
    /// radius. Defaults to `false` — a bare row is content, the shape a row
    /// hosted by [`mod@super::card_list`] (or any other container that owns
    /// the surface) needs. See the [module docs](self)' Container surface
    /// section for why this port inverts the reference's default.
    pub fn contained(mut self, contained: bool) -> Self {
        self.contained = contained;
        self
    }

    /// Gate the interactive treatment (hover/press tint and firing
    /// [`ListItem::on_press`]) and dim the container, without dropping back to
    /// a transparently-forwarding row. Defaults to `true`; only meaningful
    /// once `on_press` is chained. See the [module docs](self)' Disabled state
    /// section.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The presence-of-slots shape; a change forces a full child rebuild.
    fn shape(&self) -> (bool, bool, bool, bool) {
        (
            self.leading.is_some(),
            self.overline.is_some(),
            self.supporting.is_some(),
            self.trailing.is_some(),
        )
    }

    /// Whether the row paints a container of its own: the opt-in
    /// [`ListItem::contained`] surface, or the selection fill a
    /// [`ListItem::selected`] row must show regardless.
    fn paints_container(&self) -> bool {
        self.contained || self.selected
    }

    /// The overline text view (`on_surface_variant`, label-small, family from
    /// the theme's `labelSmall` role), or `None` when this row has no overline.
    fn overline_view(&self) -> Option<AnyView<State>> {
        self.overline.as_ref().map(|s| {
            any(text(s.clone())
                .size(OVERLINE_SIZE)
                .letter_spacing(OVERLINE_TRACKING)
                .max_lines(1)
                .overflow(TextOverflow::Ellipsis)
                .themed_role(ThemeTextColor::OnSurfaceVariant)
                .themed_family(ThemeTextType::LabelSmall))
        })
    }

    /// The headline text view (`on_surface`, default body-large size, family
    /// from the theme's `bodyLarge` role, one ellipsized line).
    fn headline_view(&self) -> AnyView<State> {
        any(text(self.headline.clone())
            .max_lines(HEADLINE_MAX_LINES)
            .overflow(TextOverflow::Ellipsis)
            .themed_family(ThemeTextType::BodyLarge))
    }

    /// The supporting text view (`on_surface_variant`, body-medium size,
    /// family from the theme's `bodyMedium` role, two ellipsized lines), or
    /// `None` when this row has no supporting line.
    fn supporting_view(&self) -> Option<AnyView<State>> {
        self.supporting.as_ref().map(|s| {
            any(text(s.clone())
                .size(SUPPORTING_SIZE)
                .max_lines(SUPPORTING_MAX_LINES)
                .overflow(TextOverflow::Ellipsis)
                .themed_role(ThemeTextColor::OnSurfaceVariant)
                .themed_family(ThemeTextType::BodyMedium))
        })
    }
}

/// Which entry of [`ListItemWidget::children`] each logical slot occupies. The
/// headline is always present; the others are optional. Storing every child in
/// one `Vec` lets a non-interactive row route events through
/// [`frust::authoring::route_event`] and recurse uniformly for paint/semantics.
#[derive(Clone, Copy)]
struct Slots {
    leading: Option<usize>,
    overline: Option<usize>,
    headline: usize,
    supporting: Option<usize>,
    trailing: Option<usize>,
}

/// The retained widget for a [`ListItem`].
pub struct ListItemWidget {
    children: Vec<ChildPod>,
    slots: Slots,
    lines: ListItemLines,
    interactive: bool,
    enabled: bool,
    selected: bool,
    /// Whether a container fill is painted at all — see
    /// [`ListItem::paints_container`].
    paints_container: bool,
    /// Armed by a `Down` (alongside `capture_pointer`), cleared on
    /// `Up`/`Cancel`/loss of interactivity or enablement.
    captured: bool,
    state: InteractionState,
    on_press: Option<ErasedCallback>,
}

/// Build the ordered child window `[leading?, overline?, headline,
/// supporting?, trailing?]` and the [`Slots`] index map, from a [`ListItem`]
/// view.
fn build_children<State: 'static>(
    view: &ListItem<State>,
    ctx: &mut BuildCtx<'_>,
) -> (Vec<ChildPod>, Slots) {
    let mut children = Vec::new();
    let leading = view.leading.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    let overline = view.overline_view().map(|v| {
        children.push(frust::authoring::build_child(&v, ctx));
        children.len() - 1
    });
    children.push(frust::authoring::build_child(&view.headline_view(), ctx));
    let headline = children.len() - 1;
    let supporting = view.supporting_view().map(|v| {
        children.push(frust::authoring::build_child(&v, ctx));
        children.len() - 1
    });
    let trailing = view.trailing.as_ref().map(|v| {
        children.push(frust::authoring::build_child(v, ctx));
        children.len() - 1
    });
    (
        children,
        Slots {
            leading,
            overline,
            headline,
            supporting,
            trailing,
        },
    )
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::card`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved state-layer content color for an interactive row. Themed:
/// `colors.on_surface`. Unthemed: [`ON_SURFACE`] exactly.
fn resolve_content_color(theme: Option<&Theme>) -> Color {
    match theme {
        Some(theme) => theme.scheme().on_surface,
        None => ON_SURFACE,
    }
}

/// The resolved container fill of a standalone row. `disabled` overrides
/// everything with the dimmed `on_surface` wash [`super::card`] uses; a
/// selected row takes `colors.secondary_container`, an ordinary one the filled
/// card role `colors.surface_container_highest`. Unthemed:
/// [`SELECTED_CONTAINER`]/[`FILLED_CONTAINER`] exactly.
fn resolve_container(theme: Option<&Theme>, selected: bool, disabled: bool) -> Color {
    if disabled {
        return with_alpha(resolve_content_color(theme), DISABLED_CONTAINER_OPACITY);
    }
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            if selected {
                scheme.secondary_container
            } else {
                scheme.surface_container_highest
            }
        }
        None => {
            if selected {
                SELECTED_CONTAINER
            } else {
                FILLED_CONTAINER
            }
        }
    }
}

/// The resolved container radius. Themed: `shape.medium`. Unthemed:
/// [`RADIUS`] exactly.
fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(theme) => theme.shape.medium,
        None => RADIUS,
    }
}

impl<State: 'static> View<State> for ListItem<State> {
    type Element = ListItemWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ListItemWidget {
        let (children, slots) = build_children(self, ctx);
        ListItemWidget {
            children,
            slots,
            lines: self.lines,
            interactive: self.on_press.is_some(),
            enabled: self.enabled,
            selected: self.selected,
            paints_container: self.paints_container(),
            captured: false,
            state: InteractionState::new(),
            on_press: self.on_press.as_ref().map(frust::authoring::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ListItemWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;

        if self.shape() != prev.shape() {
            // Slot presence changed: tear the whole child set down (against the
            // previous view, whose shape the live children still match) and
            // rebuild fresh.
            teardown_children(prev, element.slots, &mut element.children, ctx);
            let (children, slots) = build_children(self, ctx);
            element.children = children;
            element.slots = slots;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Same shape: reconcile each child in place against its prev view.
            let slots = element.slots;
            if let (Some(pi), Some(ni)) = (prev.leading.as_ref(), self.leading.as_ref())
                && let Some(idx) = slots.leading
            {
                flags |= frust::authoring::rebuild_child(pi, ni, &mut element.children[idx], ctx);
            }
            if let (Some(pv), Some(nv)) = (prev.overline_view(), self.overline_view())
                && let Some(idx) = slots.overline
            {
                flags |= frust::authoring::rebuild_child(&pv, &nv, &mut element.children[idx], ctx);
            }
            flags |= frust::authoring::rebuild_child(
                &prev.headline_view(),
                &self.headline_view(),
                &mut element.children[slots.headline],
                ctx,
            );
            if let (Some(pv), Some(nv)) = (prev.supporting_view(), self.supporting_view())
                && let Some(idx) = slots.supporting
            {
                flags |= frust::authoring::rebuild_child(&pv, &nv, &mut element.children[idx], ctx);
            }
            if let (Some(pi), Some(ni)) = (prev.trailing.as_ref(), self.trailing.as_ref())
                && let Some(idx) = slots.trailing
            {
                flags |= frust::authoring::rebuild_child(pi, ni, &mut element.children[idx], ctx);
            }
        }

        if element.lines != self.lines {
            element.lines = self.lines;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.selected != self.selected || element.paints_container != self.paints_container()
        {
            element.selected = self.selected;
            element.paints_container = self.paints_container();
            flags |= ChangeFlags::PAINT;
        }

        let now_interactive = self.on_press.is_some();
        if element.interactive != now_interactive {
            element.interactive = now_interactive;
            if !now_interactive {
                // Losing the interactive surface mid-gesture must not leave a
                // dangling capture, press, or hover behind.
                element.captured = false;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.enabled != self.enabled {
            element.enabled = self.enabled;
            if !self.enabled && element.interactive {
                // A row disabled mid-gesture keeps neither the press nor the
                // hover it was holding — mirrors `card`'s identical clear.
                element.captured = false;
                element.state.set_pressed(false);
                element.state.set_hovered(false);
            }
            flags |= ChangeFlags::PAINT;
        }
        element.on_press = self.on_press.as_ref().map(frust::authoring::erase_callback);
        flags
    }

    fn teardown(&self, element: &mut ListItemWidget, ctx: &mut BuildCtx<'_>) {
        teardown_children(self, element.slots, &mut element.children, ctx);
    }
}

/// Tear down every child pod through the view it was built from, per the
/// [`Slots`] index map. `teardown_child` cancels an in-flight capture regardless
/// of view type; the reconstructed text views only need to match the pod's
/// concrete `TextWidget` type for the (usually no-op) `teardown` dispatch.
fn teardown_children<State: 'static>(
    view: &ListItem<State>,
    slots: Slots,
    children: &mut [ChildPod],
    ctx: &mut BuildCtx<'_>,
) {
    for (index, pod) in children.iter_mut().enumerate() {
        if Some(index) == slots.leading
            && let Some(v) = view.leading.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        } else if Some(index) == slots.overline
            && let Some(v) = view.overline_view()
        {
            frust::authoring::teardown_child(&v, pod, ctx);
        } else if index == slots.headline {
            frust::authoring::teardown_child(&view.headline_view(), pod, ctx);
        } else if Some(index) == slots.supporting
            && let Some(v) = view.supporting_view()
        {
            frust::authoring::teardown_child(&v, pod, ctx);
        } else if Some(index) == slots.trailing
            && let Some(v) = view.trailing.as_ref()
        {
            frust::authoring::teardown_child(v, pod, ctx);
        }
    }
}

impl ListItemWidget {
    /// The y origin a slot pins to for a `slot`-tall child: the top inset on a
    /// three-line row (`crossAxisAlignment: start`), centered otherwise.
    fn slot_y(&self, row_height: f64, slot_height: f64) -> f64 {
        if self.lines == ListItemLines::Three {
            VPAD_THREE_LINE
        } else {
            ((row_height - slot_height) / 2.0).max(0.0)
        }
    }
}

impl Widget for ListItemWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let height = self.lines.height();
        let slot_bc = BoxConstraints::new(Size::ZERO, Size::new(width, height));

        // Leading slot at the left inset.
        let mut text_left = HPAD;
        if let Some(idx) = self.slots.leading {
            let s = self.children[idx].layout_child(ctx, &slot_bc);
            let y = self.slot_y(height, s.height);
            self.children[idx].set_origin(Point::new(HPAD, y));
            text_left = HPAD + s.width + GAP;
        }

        // Trailing slot at the right inset.
        let mut text_right = width - HPAD;
        if let Some(idx) = self.slots.trailing {
            let s = self.children[idx].layout_child(ctx, &slot_bc);
            let x = width - HPAD - s.width;
            let y = self.slot_y(height, s.height);
            self.children[idx].set_origin(Point::new(x, y));
            text_right = x - GAP;
        }

        // Text column between the slots.
        let text_w = (text_right - text_left).max(0.0);
        let text_bc = BoxConstraints::new(Size::ZERO, Size::new(text_w, height));
        let over_size = if let Some(oi) = self.slots.overline {
            self.children[oi].layout_child(ctx, &text_bc)
        } else {
            Size::ZERO
        };
        let hi = self.slots.headline;
        let head_size = self.children[hi].layout_child(ctx, &text_bc);
        let supp_size = if let Some(si) = self.slots.supporting {
            self.children[si].layout_child(ctx, &text_bc)
        } else {
            Size::ZERO
        };

        // Vertically center the overline+headline+supporting block within the
        // row (the fixed row heights already bake in the vertical padding, so a
        // block that fits leaves >=8dp above and below).
        let block = over_size.height + head_size.height + supp_size.height;
        let mut y = ((height - block) / 2.0).max(0.0);
        if let Some(oi) = self.slots.overline {
            self.children[oi].set_origin(Point::new(text_left, y));
            y += over_size.height;
        }
        self.children[hi].set_origin(Point::new(text_left, y));
        y += head_size.height;
        if let Some(si) = self.slots.supporting {
            self.children[si].set_origin(Point::new(text_left, y));
        }

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every theme read happens before `ctx` is taken mutably below
        // (`paint_child`) — mirrors `card::CardWidget::paint`'s ordering.
        let theme = Theme::from_paint_ctx(ctx);
        let disabled = self.interactive && !self.enabled;
        // A row with no container of its own tints square: the host owns
        // whatever corner shape is under it, and a 12dp overlay corner inside
        // a differently-rounded host pokes out at the corners.
        let radius = if self.paints_container {
            resolve_radius(theme)
        } else {
            0.0
        };
        let container = self
            .paints_container
            .then(|| resolve_container(theme, self.selected, disabled));
        let content_color = resolve_content_color(theme);
        // The pod's hover link is authoritative; the flag the `Move` arm
        // latched is only what earns the row a frame when hover *begins*. A
        // pointer that left the row routed its next move elsewhere, so no event
        // ever told this row it stopped being hovered — self-correcting the flag
        // here is what makes the overlay drop on the very frame the pointer
        // moves onto a sibling. Enabled-gated: a disabled row never reacts to
        // hover.
        let hovered = self.interactive && self.enabled && ctx.is_hovered();

        let o = ctx.origin();
        let size = ctx.size();

        self.state.set_hovered(hovered);

        if let Some(container) = container {
            scene.fill_rounded_rect(o, size, radius, container);
        }
        if self.interactive && self.enabled {
            let opacity = self.state.resolve_opacity();
            if opacity > 0.0 {
                scene.fill_rounded_rect(o, size, radius, with_alpha(content_color, opacity));
            }
        }
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.interactive {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        }
        // Non-pointer events (Key, Ime, focus-routed) must be forwarded to
        // children, even when interactive. Only pointer events drive the
        // interactive row's own capture/press behavior.
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event(&mut self.children, ctx, event);
        };
        if !self.enabled {
            // A disabled interactive row arms nothing and claims no hover, and
            // does not forward to its slots either (see the module docs'
            // Disabled state section) — mirrors `card`'s disabled early-return.
            return EventResult::Ignored;
        }
        let on_press = self
            .on_press
            .as_mut()
            .expect("on_press is set whenever interactive is true");
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(true);
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    // No capture: this is the hover pass. Claim the link whenever
                    // the pointer is inside the row — every qualifying move, not
                    // just the first, since a claim covers only its own pass. The
                    // redraw is gated on the setter's changed-return: that gating is
                    // what paints the overlay on entry (a claim asks for no frame)
                    // while a pointer wandering *within* the row costs nothing after
                    // the first move. A claim made while some pointer is captured
                    // (this row's or anyone's) is refused by the framework, so no
                    // drag can reach this arm and tint the row; an uncaptured touch
                    // drag can, and its lift's `Up` ends the link.
                    let over = inside(p.position, ctx.size());
                    if over {
                        ctx.claim_hover();
                    }
                    if self.state.set_hovered(over) {
                        ctx.request_redraw();
                    }
                    // Still `Ignored`: watching a move is not consuming it.
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                if self.state.set_pressed(inside_now) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (on_press)(ctx);
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.state.set_pressed(false);
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::ListItem,
            |node| {
                if self.interactive {
                    if self.enabled {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                }
                // Set only while selected: accesskit's own guidance is that the
                // flag's absence means "selection doesn't apply here", and a
                // `false` earns an extraneous "not selected" announcement.
                if self.selected {
                    node.set_selected(true);
                }
            },
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use frust_widgets::test_support::leaf;
    use kurbo::Point;
    use std::any::Any;

    fn build<S: 'static>(view: &ListItem<S>) -> ListItemWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ListItemWidget, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    /// Records each rounded rect's `(origin, size, radius, color)` — the
    /// container fill and the state-layer overlay both land here.
    #[derive(Default)]
    struct RRectRecorder {
        rrects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for RRectRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
    }

    impl RRectRecorder {
        /// Every translucent rect painted this frame — the state-layer
        /// overlays, told apart from the opaque container fills by alpha.
        fn overlays(&self) -> Vec<(Point, f32)> {
            self.rrects
                .iter()
                .filter(|(_, _, _, c)| c.components[3] < 1.0)
                .map(|(o, _, _, c)| (*o, c.components[3]))
                .collect()
        }
    }

    fn paint(w: &mut ListItemWidget, size: Size, theme: Option<&Theme>) -> RRectRecorder {
        let mut rec = RRectRecorder::default();
        let mut ctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn one_line_row_is_56dp() {
        let view: ListItem<()> = list_item("Title");
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, ONE_LINE_HEIGHT);
    }

    #[test]
    fn supporting_promotes_to_two_line_72dp() {
        let view: ListItem<()> = list_item("Title").supporting("Subtitle");
        assert_eq!(view.lines, ListItemLines::Two);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, TWO_LINE_HEIGHT);
    }

    #[test]
    fn overline_alone_promotes_to_two_line_and_with_supporting_to_three() {
        // The reference's `_isThreeLine`: overline AND supporting.
        let two: ListItem<()> = list_item("Title").overline("OVERLINE");
        assert_eq!(two.lines, ListItemLines::Two);

        let three: ListItem<()> = list_item("Title").overline("OVERLINE").supporting("Sub");
        assert_eq!(three.lines, ListItemLines::Three);

        // Order-independent, and an explicit `three_line()` is never demoted.
        let flipped: ListItem<()> = list_item("Title").supporting("Sub").overline("OVERLINE");
        assert_eq!(flipped.lines, ListItemLines::Three);
        let forced: ListItem<()> = list_item("Title").three_line().supporting("Sub");
        assert_eq!(forced.lines, ListItemLines::Three);
    }

    #[test]
    fn three_line_is_88dp() {
        let view: ListItem<()> = list_item("Title").supporting("Sub").three_line();
        assert_eq!(view.lines, ListItemLines::Three);
        let mut w = build(&view);
        let size = layout(&mut w, 300.0);
        assert_eq!(size.height, THREE_LINE_HEIGHT);
    }

    #[test]
    fn leading_and_trailing_slots_are_placed_and_inset_the_text() {
        // A 24x24 leading and a 16x16 trailing leaf, so the text column starts
        // after the leading + gap and ends before the trailing + gap.
        let view: ListItem<()> = list_item("Title")
            .leading(leaf(24.0, 24.0))
            .trailing(leaf(16.0, 16.0));
        let mut w = build(&view);
        layout(&mut w, 300.0);

        let leading_i = w.slots.leading.expect("leading present");
        let trailing_i = w.slots.trailing.expect("trailing present");
        let headline_i = w.slots.headline;

        // Leading sits at the 16dp inset, vertically centered in the 56dp row.
        assert_eq!(w.children[leading_i].origin().x, HPAD);
        assert_eq!(
            w.children[leading_i].origin().y,
            (ONE_LINE_HEIGHT - 24.0) / 2.0
        );
        // Trailing sits at the right inset.
        assert_eq!(w.children[trailing_i].origin().x, 300.0 - HPAD - 16.0);
        // The headline is pushed right of the leading + gap.
        assert_eq!(w.children[headline_i].origin().x, HPAD + 24.0 + GAP);
    }

    #[test]
    fn a_three_line_rows_slots_pin_to_the_top_inset() {
        // `crossAxisAlignment: threeLine ? start : center` — a tall slot on a
        // three-line row starts at the 12dp inset instead of centering.
        let view: ListItem<()> = list_item("Title")
            .overline("OVER")
            .supporting("Sub")
            .leading(leaf(40.0, 40.0));
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let leading_i = w.slots.leading.expect("leading present");
        assert_eq!(w.children[leading_i].origin().y, VPAD_THREE_LINE);
    }

    #[test]
    fn build_orders_children_leading_overline_headline_supporting_trailing() {
        let view: ListItem<()> = list_item("H")
            .overline("O")
            .supporting("S")
            .leading(leaf(10.0, 10.0))
            .trailing(leaf(10.0, 10.0));
        let w = build(&view);
        assert_eq!(w.children.len(), 5);
        assert_eq!(w.slots.leading, Some(0));
        assert_eq!(w.slots.overline, Some(1));
        assert_eq!(w.slots.headline, 2);
        assert_eq!(w.slots.supporting, Some(3));
        assert_eq!(w.slots.trailing, Some(4));
    }

    #[test]
    fn the_text_column_stacks_overline_headline_supporting_in_order() {
        let view: ListItem<()> = list_item("Headline")
            .overline("OVERLINE")
            .supporting("Supporting");
        let mut w = build(&view);
        layout(&mut w, 300.0);
        let over = w.children[w.slots.overline.expect("overline")].origin().y;
        let head = w.children[w.slots.headline].origin().y;
        let supp = w.children[w.slots.supporting.expect("supporting")]
            .origin()
            .y;
        assert!(over < head, "overline sits above the headline");
        assert!(head < supp, "supporting sits below the headline");
    }

    #[test]
    fn every_slot_combination_x_state_builds_lays_out_and_paints() {
        // The reference's whole slot surface (`leading`/`overline`/
        // `supporting`/`trailing`, each optional) crossed with the states that
        // change what is painted. Each case must reach a laid-out row of its
        // declared height whose children all fit inside it, and paint at most
        // one container plus one state layer.
        let theme = crate::baseline();
        for leading in [false, true] {
            for overline in [false, true] {
                for supporting in [false, true] {
                    for trailing in [false, true] {
                        for (selected, contained, interactive) in [
                            (false, false, false),
                            (false, true, false),
                            (true, false, false),
                            (true, true, true),
                            (false, false, true),
                        ] {
                            let mut view: ListItem<()> = list_item("Headline");
                            if leading {
                                view = view.leading(leaf(24.0, 24.0));
                            }
                            if overline {
                                view = view.overline("OVERLINE");
                            }
                            if supporting {
                                view = view.supporting("Supporting text");
                            }
                            if trailing {
                                view = view.trailing(leaf(16.0, 16.0));
                            }
                            if interactive {
                                view = view.on_press(|_: &mut ()| {});
                            }
                            let expected_height = view.lines.height();
                            let view = view.selected(selected).contained(contained);

                            let mut w = build(&view);
                            let size = layout(&mut w, 300.0);
                            let case = format!(
                                "leading={leading} overline={overline} \
                                 supporting={supporting} trailing={trailing} \
                                 selected={selected} contained={contained} \
                                 interactive={interactive}"
                            );
                            assert_eq!(size.height, expected_height, "{case}");
                            for pod in &w.children {
                                assert!(
                                    pod.origin().y >= 0.0
                                        && pod.origin().y + pod.size().height <= size.height + 0.01,
                                    "a child escapes the row vertically ({case})"
                                );
                            }
                            let rec = paint(&mut w, size, Some(&theme));
                            let expected_rects = usize::from(selected || contained);
                            assert_eq!(
                                rec.rrects.len(),
                                expected_rects,
                                "container fill count ({case})"
                            );
                        }
                    }
                }
            }
        }
    }

    // --- Container surface ---

    #[test]
    fn a_bare_row_paints_no_container_of_its_own() {
        // The default: a row is content, and whatever hosts it owns the
        // surface (the shape the reference's `M3EListItemScope` rows render).
        let view: ListItem<()> = list_item("Title");
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), None);
        assert!(rec.rrects.is_empty());
    }

    #[test]
    fn a_contained_row_paints_the_filled_container_at_the_medium_radius() {
        let view: ListItem<()> = list_item("Title").contained(true);
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), None);
        assert_eq!(rec.rrects.len(), 1, "just the container fill at rest");
        assert_eq!(rec.rrects[0].3, FILLED_CONTAINER);
        assert_eq!(rec.rrects[0].2, RADIUS);
    }

    #[test]
    fn a_selected_row_paints_the_secondary_container_fill_with_or_without_contained() {
        let theme = crate::baseline();
        // Selection is a state the user must see, so it paints its fill even
        // on an otherwise-bare row.
        let view: ListItem<()> = list_item("Title").selected(true);
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), Some(&theme));
        assert_eq!(rec.rrects.len(), 1);
        assert_eq!(rec.rrects[0].3, theme.scheme().secondary_container);
        assert_eq!(rec.rrects[0].2, theme.shape.medium);

        let contained: ListItem<()> = list_item("Title").contained(true).selected(true);
        let mut w = build(&contained);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().secondary_container);

        let unselected: ListItem<()> = list_item("Title").contained(true);
        let mut w = build(&unselected);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_highest);
    }

    #[test]
    fn a_disabled_interactive_row_dims_its_container_and_paints_no_overlay() {
        let theme = crate::baseline();
        let view: ListItem<()> = list_item("Title")
            .contained(true)
            .on_press(|_: &mut ()| {})
            .enabled(false);
        let mut w = build(&view);
        let rec = paint(&mut w, Size::new(300.0, ONE_LINE_HEIGHT), Some(&theme));
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(theme.scheme().on_surface, DISABLED_CONTAINER_OPACITY)
        );
        // The dim wash is itself translucent, so count rects rather than
        // filtering by alpha: one container fill, no state layer on top.
        assert_eq!(rec.rrects.len(), 1, "no state layer while disabled");
    }

    // --- Interactivity ---

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn dispatch<S: 'static>(
        w: &mut ListItemWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(300.0, ONE_LINE_HEIGHT));
        w.event(&mut ctx, event)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Primary,
        })
    }

    /// The same event on the secondary (right) button.
    fn secondary_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust::authoring::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust::authoring::PointerButton::Secondary,
        })
    }

    #[test]
    fn interactive_row_fires_on_up_inside() {
        let view: ListItem<Counter> =
            list_item("Tap me").on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn a_secondary_press_never_presses_captures_or_fires() {
        let view: ListItem<Counter> =
            list_item("Tap me").on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        let r = dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Down, 10.0, 10.0),
        );
        assert_eq!(r, EventResult::Ignored);
        assert!(!w.state.pressed, "no pressed state layer on a right-click");
        assert!(!w.captured, "and no capture for the shell to wedge on");
        dispatch(
            &mut w,
            &mut state,
            &secondary_ev(PointerPhase::Up, 10.0, 10.0),
        );
        assert_eq!(state.presses, 0);

        // The primary gesture is untouched by the guard.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 1);
    }

    #[test]
    fn interactive_row_up_outside_does_not_fire() {
        let view: ListItem<Counter> =
            list_item("Tap me").on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 999.0, 999.0));
        assert_eq!(state.presses, 0);
    }

    #[test]
    fn a_disabled_interactive_row_ignores_every_pointer_phase() {
        let view: ListItem<Counter> = list_item("Tap me")
            .on_press(|s: &mut Counter| s.presses += 1)
            .enabled(false);
        let mut w = build(&view);
        let mut state = Counter::default();
        assert_eq!(
            dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0)),
            EventResult::Ignored
        );
        assert!(!w.captured, "a disabled row never captures");
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.presses, 0, "and never fires");
    }

    // --- Interactive with focus-routed child events ---

    /// A minimal widget that handles Key events for testing focus routing.
    struct FocusConsumer;

    impl FocusConsumer {
        fn new() -> Self {
            FocusConsumer
        }
    }

    struct FocusConsumerWidget;

    impl View<Counter> for FocusConsumer {
        type Element = FocusConsumerWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> FocusConsumerWidget {
            FocusConsumerWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut FocusConsumerWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for FocusConsumerWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            // Handle any key event (focus-routed events reach here only if the
            // pod is focused, so a Key event here proves the routing worked).
            if let InputEvent::Key(_) = event {
                return EventResult::Handled;
            }
            EventResult::Ignored
        }
    }

    #[test]
    fn interactive_row_with_trailing_control_forwards_key_events() {
        let view: ListItem<Counter> = list_item("Item")
            .trailing(FocusConsumer::new())
            .on_press(|s: &mut Counter| s.presses += 1);
        let mut w = build(&view);
        let mut state = Counter::default();

        // Manually set the trailing control as focused to simulate a prior focus state.
        // (In real usage, the control would be focused by a pointer-down event,
        // but here we're directly testing the event-routing path.)
        let trailing_idx = w.slots.trailing.expect("trailing is present");
        w.children[trailing_idx].set_focused(true);

        // Send a Key event — it should be forwarded to the focused child.
        let key_event = InputEvent::Key(frust::authoring::KeyEvent {
            key: frust::authoring::Key::Named(frust::authoring::NamedKey::Backspace),
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &key_event);

        // Verify the key event was handled (forwarded to and handled by child).
        assert_eq!(
            result,
            EventResult::Handled,
            "key event must be forwarded to focused child"
        );

        // The row press should not have fired (the row only fires on pointer Up).
        assert_eq!(
            state.presses, 0,
            "row press callback does not fire on key event"
        );
    }

    // --- Hover ---

    /// Two interactive rows in a `Column` under a real `RenderRoot` — the only
    /// harness that can exercise hover at all, since the hover link is recorded by
    /// the root's event pass and read back through `PaintCtx::is_hovered`. It is
    /// also the out-of-tree reachability proof: everything here is reached through
    /// the `frust` facade, exactly as a third-party catalog would.
    struct HoverHarness {
        root: frust_core::RenderRoot<Counter, frust::FlexView<Counter>>,
        state: Counter,
        tcx: TextContext,
    }

    /// Window/row geometry: two stacked 56dp rows in a 300x200 window.
    const HOVER_WINDOW: Size = Size::new(300.0, 200.0);

    impl HoverHarness {
        fn new() -> Self {
            let mut h = HoverHarness {
                root: frust_core::RenderRoot::new(),
                state: Counter::default(),
                tcx: TextContext::new(),
            };
            h.rebuild_layout();
            h
        }

        fn rebuild_layout(&mut self) {
            let mut app = |_s: &mut Counter| {
                frust::Column(vec![
                    frust::authoring::any(
                        list_item::<Counter>("first").on_press(|s: &mut Counter| s.presses += 1),
                    ),
                    frust::authoring::any(
                        list_item::<Counter>("second").on_press(|s: &mut Counter| s.presses += 1),
                    ),
                ])
            };
            self.root.rebuild(&mut app, &mut self.state);
            self.root
                .layout_with_text(HOVER_WINDOW, &mut self.tcx as &mut dyn Any);
        }

        fn dispatch(
            &mut self,
            phase: PointerPhase,
            x: f64,
            y: f64,
        ) -> frust::authoring::EventOutcome {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(frust::authoring::PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: frust::authoring::PointerButton::Primary,
                }),
            )
        }

        /// The y-centre of row `index` (rows are `ONE_LINE_HEIGHT` tall).
        fn row_y(index: usize) -> f64 {
            ONE_LINE_HEIGHT * index as f64 + ONE_LINE_HEIGHT / 2.0
        }

        /// Paint and report which rows painted a state-layer overlay, by the
        /// overlay rect's y origin. The opaque container fills every row paints
        /// are filtered out by alpha.
        fn overlay_rows(&mut self) -> Vec<usize> {
            let mut rec = RRectRecorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.overlays()
                .iter()
                .map(|(origin, _)| (origin.y / ONE_LINE_HEIGHT).round() as usize)
                .collect()
        }

        /// The alpha of the single overlay painted this frame.
        fn overlay_alpha(&mut self) -> f32 {
            let mut rec = RRectRecorder::default();
            self.root.paint(&mut rec, frust::FrameTime::ZERO);
            rec.overlays().first().expect("one overlay painted").1
        }
    }

    #[test]
    fn hovering_a_row_paints_the_hover_overlay() {
        let mut h = HoverHarness::new();
        assert!(h.overlay_rows().is_empty(), "no overlay at rest");

        // The frame that makes the overlay appear is the row's own: `claim_hover`
        // requests none, and the pipeline manufactures one only for a hover that
        // ended, so the row's change-gated `request_redraw` is what asks here.
        let gain = h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        assert!(gain.needs_redraw, "entering a row repaints");
        assert_eq!(h.overlay_rows(), vec![0], "the hovered row tints");
        assert_eq!(
            h.overlay_alpha(),
            crate::interaction::HOVER_OPACITY,
            "at the documented M3 hover opacity"
        );

        // Moving within the same row re-claims but changes nothing.
        let settled = h.dispatch(PointerPhase::Move, 160.0, HoverHarness::row_y(0));
        assert!(!settled.needs_redraw, "an unchanged flag asks for nothing");
        assert_eq!(h.overlay_rows(), vec![0]);
    }

    #[test]
    fn hover_moves_to_the_row_under_the_pointer() {
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_rows(), vec![0]);

        // The row the pointer left never receives an event about it — the move
        // routes to its sibling — so this is the case a widget cannot handle on
        // its own, and exactly one row may end up tinted. The arriving row's own
        // flag change is what asks for the frame (the root's hover mirror is
        // identity-free and sees `true` → `true` across a handoff); a repaint being
        // global is what lets the departing row drop its tint in the same frame.
        let handoff = h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(1));
        assert!(handoff.needs_redraw, "a row-to-row handoff repaints");
        assert_eq!(h.overlay_rows(), vec![1]);

        // Off both rows: nothing tints, and the row that lost hover gets the
        // repaint it could not ask for itself.
        let outcome = h.dispatch(PointerPhase::Move, 150.0, 180.0);
        assert!(outcome.needs_redraw);
        assert!(h.overlay_rows().is_empty());
    }

    #[test]
    fn a_captured_drag_paints_no_hover_overlay() {
        let mut h = HoverHarness::new();
        // Press row 0: the row captures, and the press overlay (10%) is what
        // shows — never the hover one.
        h.dispatch(PointerPhase::Down, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_rows(), vec![0]);
        assert_eq!(h.overlay_alpha(), frust::authoring::PRESSED_OPACITY);

        // Drag off the row: the captured row keeps receiving moves and its own
        // `Move` arm calls `claim_hover()`, which must record nothing.
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(1));
        assert!(
            h.overlay_rows().is_empty(),
            "dragged outside: neither pressed nor hovered"
        );

        // Release outside: no press fires, and no hover was left behind.
        h.dispatch(PointerPhase::Up, 150.0, HoverHarness::row_y(1));
        assert_eq!(h.state.presses, 0);
        assert!(h.overlay_rows().is_empty());
    }

    #[test]
    fn hover_survives_a_rebuild_and_clears_on_press() {
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(0));
        h.rebuild_layout();
        assert_eq!(
            h.overlay_rows(),
            vec![0],
            "an in-place rebuild keeps the pod that holds the link"
        );

        // A `Down` ends the hover link outright; the press overlay takes over.
        h.dispatch(PointerPhase::Down, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.overlay_alpha(), frust::authoring::PRESSED_OPACITY);
        h.dispatch(PointerPhase::Up, 150.0, HoverHarness::row_y(0));
        assert_eq!(h.state.presses, 1);
        assert!(
            h.overlay_rows().is_empty(),
            "a click leaves no hover behind until the pointer moves again"
        );
    }

    #[test]
    fn an_uncaptured_drag_tints_the_row_only_until_the_lift() {
        // The touch shape: a contact that started on empty chrome captures nothing,
        // so its moves are ordinary hover passes and the row it slides over does
        // tint — nothing in the pipeline tells a finger from a mouse. The lift is
        // what bounds it: a lifted contact sends no further move, so `Up` ending the
        // hover link is the only thing that can drop the tint.
        let mut h = HoverHarness::new();
        h.dispatch(PointerPhase::Down, 150.0, 180.0);
        assert!(h.overlay_rows().is_empty(), "pressed empty chrome, no row");

        h.dispatch(PointerPhase::Move, 150.0, HoverHarness::row_y(1));
        assert_eq!(
            h.overlay_rows(),
            vec![1],
            "an uncaptured drag over the row is a hover pass"
        );

        let lift = h.dispatch(PointerPhase::Up, 150.0, HoverHarness::row_y(1));
        assert!(
            lift.needs_redraw,
            "the lift asks for the frame that clears it"
        );
        assert!(
            h.overlay_rows().is_empty(),
            "the tint is transient: the lift ends the link"
        );
        assert_eq!(h.state.presses, 0, "no press fired: the row never captured");
    }

    // --- Semantics ---

    #[test]
    fn semantics_row_is_a_list_item_with_the_headline_label() {
        fn logic(_s: &mut ()) -> ListItem<()> {
            list_item("Inbox")
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row contributes a Role::ListItem node");
        assert!(
            !item.children().is_empty(),
            "the headline text is a semantics child of the row"
        );
        // The headline run's text is carried on a Label node.
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.value() == Some("Inbox")),
            "the headline text is announced"
        );
        assert_eq!(
            item.is_selected(),
            None,
            "an unselected row omits the flag entirely (accesskit's own guidance)"
        );
    }

    #[test]
    fn interactive_row_semantics_carries_a_click_action() {
        fn logic(_s: &mut ()) -> ListItem<()> {
            list_item("Go").on_press(|_s: &mut ()| {})
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row node");
        assert!(
            item.supports_action(Action::Click),
            "an interactive row exposes the Click action"
        );
        assert!(!item.is_disabled());
    }

    #[test]
    fn semantics_disabled_row_reports_disabled_and_selected_row_reports_selection() {
        fn disabled_logic(_s: &mut ()) -> ListItem<()> {
            list_item("Go").on_press(|_s: &mut ()| {}).enabled(false)
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut disabled_logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row node");
        assert!(item.is_disabled());
        assert!(!item.supports_action(Action::Click));

        fn selected_logic(_s: &mut ()) -> ListItem<()> {
            list_item("Go").selected(true)
        }
        let mut root: RenderRoot<(), ListItem<()>> = RenderRoot::new();
        root.rebuild(&mut selected_logic, &mut state);
        root.layout_with_text(Size::new(300.0, 100.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, item) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ListItem)
            .expect("row node");
        assert_eq!(item.is_selected(), Some(true));
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn every_text_slot_paints_in_roboto_flex_under_the_material_theme() {
        // Overline, headline and supporting line — all three text slots a
        // row builds from plain `text(..)`. See
        // `crate::appbar::top::typeface_probe`.
        use crate::appbar::top::typeface_probe::{Face, assert_paints_only_in};
        let flex = crate::tokens::font_data()[0];
        let runs = assert_paints_only_in(
            "the list item's text slots",
            |_: &mut ()| {
                list_item::<()>("Headline")
                    .overline("Overline")
                    .supporting("Supporting text")
            },
            crate::baseline(),
            &[flex],
            Size::new(300.0, 100.0),
            Face {
                bytes: flex,
                name: "Roboto Flex",
            },
        );
        assert!(
            runs >= 3,
            "fixture sanity: expected a run per text slot, got {runs}"
        );
    }
}
