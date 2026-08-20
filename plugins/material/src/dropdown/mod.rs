// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/dropdown_menus/` — `m3e_dropdown_menus.dart` (the widget's
// prop set), `controllers/m3e_dropdown_controller.dart` (selection + filter
// rules), `components/` (`m3e_dropdown_menu_field.dart`,
// `m3e_dropdown_menu_panel.dart`, `m3e_dropdown_menu_item.dart`,
// `m3e_dropdown_chips.dart`, `m3e_dropdown_menu_actions.dart`,
// `m3e_dropdown_menu_lifecycle.dart`, `m3e_dropdown_menu_chip_build.dart`) and
// `styles/` (`m3e_dropdown_menu_theme.dart` plus the five per-part styles),
// retrieved 2026-08-20. That family is itself upstream's vendoring of
// <https://github.com/Mudit200408/m3e_dropdown_menu>.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
// Porting decisions (each expanded in the module docs below): the reference's
// `FormField` wrapper — `validator`, `autovalidateMode`, the inline error line
// and Flutter's restoration plumbing — is **descoped**; the reference's own
// `OverlayPortal` + expand/collapse spring is **not** ported (this catalog
// already merged one anchored host, `crate::overlay::anchored`, which owns
// placement, light dismiss, Escape and the enter/exit ramp); `M3EDropdownMenu
// .future` becomes a poll seam (`loading` + app-fed items); and selection lives
// with the app rather than on the item, per this catalog's controlled-component
// rule.

//! The Material 3 Expressive **dropdown menu**: a text-field-shaped trigger and
//! the option panel it anchors — single-select, multi-select with chips inside
//! the field, type-to-filter search, and async option loading.
//!
//! # Two views, because there is no portal
//!
//! A widget paints inside its parent's box, so the panel cannot live inside the
//! field. The family is therefore the same two-piece shape [`mod@crate::menu`]
//! takes:
//!
//! 1. [`dropdown_field`] — the trigger, mounted in the app's own layout. It
//!    captures its window-space rect into an [`OverlayAnchor`] on every paint
//!    (the same one-line capture [`crate::overlay::overlay_anchor`] performs,
//!    inlined so a field needs no wrapper).
//! 2. [`dropdown`] — [`dropdown_panel`] inside [`crate::overlay::anchored`],
//!    mounted as the top child of a full-area [`frust::Stack`] (or a
//!    transparent navigator page), pointed at the same anchor.
//!
//! Both take the same `items`/`selected` props, and both are **controlled**:
//! neither ever mutates a selection, an open flag or a query of its own. Every
//! interaction reports the value it *requests* and waits for the app to feed it
//! back down — the catalog-wide rule (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics), and the reason [`DropdownItem`] carries no `selected` field even
//! though the reference's `M3EDropdownItem` does.
//!
//! # The host owns the ramp
//!
//! The reference drives its own `_expandCtrl`/`_arrowCtrl` springs and inserts
//! its own `OverlayPortal` with a hand-rolled outside-tap `Listener`
//! (`m3e_dropdown_menu_panel.dart:5`, `m3e_dropdown_menu_actions.dart:5`).
//! **None of the panel half is ported**: [`crate::overlay::anchored`] already
//! places the panel, light-dismisses, handles Escape and composites the
//! fade + anchor-pivoted scale. The one spring that *is* ported is the field's
//! arrow rotation, which is the trigger's own chrome — see [`mod@field`].
//!
//! An app that wants the panel's **exit** ramp keeps [`dropdown`] mounted and
//! toggles [`DropdownView::open`] rather than unmounting it (the kept-mounted
//! pattern that host documents).
//!
//! # Descoped: form-field validation and restoration
//!
//! The reference wraps its whole tree in a Flutter `FormField`
//! (`m3e_dropdown_menus.dart:355`) for `validator`/`autovalidateMode`, renders
//! an error line under the field, and recolors the field's border through
//! `formState.hasError`. **None of that ships here**: this workspace has no
//! `Form`/`FormField` equivalent to register with, and Flutter's state
//! restoration has no counterpart at all. An app that needs validation renders
//! its own message beside the field and drives [`DropdownFieldView::error`] for
//! the error-colored container.
//!
//! # Async options: a poll seam, not a `Future`
//!
//! `M3EDropdownMenu.future` owns the load: it calls the provider in `initState`,
//! flips a private `_isLoading`, and writes the result into its own controller
//! (`m3e_dropdown_menu_lifecycle.dart:128`). A controlled widget cannot own
//! that, and this crate has no established async widget seam (neither
//! [`mod@crate::text_field`] nor [`mod@crate::slider`] has one). The adaptation
//! is a **poll seam**: the app loads options however it likes
//! (`frust::use_task`, `frust::spawn`, a plugin call) and feeds
//! [`DropdownView::loading`], [`DropdownView::error`] and the `items` list back
//! down. While `loading` is set the panel shows the reference's centered
//! indicator row and the field's trailing slot swaps its arrow for a small one,
//! exactly as `_buildFieldTrailing`/`_buildPanelBody` do; the field also refuses
//! to open, matching `_toggle`'s `if (!widget.enabled || _isLoading) return`.
//!
//! # Divergences from [`mod@crate::menu`]'s item rendering
//!
//! The row vocabulary is shared — rows are **data** with cached shaped runs and
//! parsed icon paths, not child pods, so per-state ink stays expressible — but
//! the reference's dropdown item is a different control from its menu item, so
//! nothing is reused verbatim:
//!
//! | | `crate::menu` row | dropdown row |
//! |---|---|---|
//! | container | transparent, on one elevated surface | its own `surfaceContainerHigh` card |
//! | corners | one `item_radius` (12) | the M3E card-list treatment: [`DROPDOWN_ITEM_OUTER_RADIUS`] caps, [`DROPDOWN_ITEM_INNER_RADIUS`] inside, morphing on hover/press |
//! | selected fill | `tertiaryContainer` | `secondaryContainer` |
//! | label role | `labelLarge` | `bodyLarge` |
//! | selected check | **leading**, at 90% icon size | **trailing**, at full icon size |
//! | extra slots | supporting text, shortcut, submenu chevron | none |
//!
//! The corner rule is literally the card list's: this module resolves it
//! through [`crate::card_list::card_position`]/[`crate::card_list::card_radii`],
//! which is what the reference's own comment points at
//! (`m3e_dropdown_item_style.dart:39`, *"mirroring the M3E card list
//! treatment"*). One reference quirk is kept: a **selected** row takes a uniform
//! radius and loses its cap corners entirely (`_calculateBaseRadius`'s first
//! branch short-circuits before the first/last cases).
//!
//! The one addition over the reference: a row paints this catalog's shared
//! [`InteractionState`](crate::interaction::InteractionState) state layer. The
//! reference delegates its overlay to `M3ECard`, whose tappable surface paints
//! one for it; resolving it here keeps the row a plain painted band.
//!
//! # Divergences from [`mod@crate::chips`]
//!
//! The multi-select chips inside the field are **not** [`crate::input_chip`]s.
//! The reference ships its own `M3ESpringChip` (`m3e_dropdown_chips.dart`) with
//! its own metrics, and they differ from `M3EChipTheme`'s on every axis but the
//! color pair:
//!
//! | | `crate::chips` input chip | dropdown chip |
//! |---|---|---|
//! | corner radius | 8dp (`shape.small`) | [`DROPDOWN_CHIP_RADIUS`], 20dp |
//! | horizontal insets | 16dp leading / 12dp trailing | [`DROPDOWN_CHIP_H_PADDING`] both sides |
//! | height | a fixed 32dp | its label's line box plus [`DROPDOWN_CHIP_V_PADDING`], so the strip breathes with the type scale |
//! | label role | the nested text's themed role | `labelMedium` |
//! | fill / ink | `secondaryContainer` / `onSecondaryContainer` | the same pair |
//!
//! They are painted by the field rather than mounted as `input_chip` pods for
//! two reasons: a pod cannot be spring-scaled about its own leading edge from
//! the field's paint pass, and a removed chip has to outlive the rebuild that
//! dropped it to play its exit. The one place this port takes the chip
//! catalog's number over the reference's is the delete glyph
//! ([`DROPDOWN_CHIP_ICON_SIZE`]) — see that constant.
//!
//! The reference's chip choreography is ported **in part**: the spring
//! entrance and exit are here; the sibling *slide* + *squish* micro-animations
//! that follow an insertion or a removal (`_triggerInsertionSquish`,
//! `_animateChipsAfterRemoval`) are not, and neither is the non-wrapping
//! `Flow`-delegate row they exist to drive (`chipStyle.wrap` defaults to `true`,
//! which is the mode ported here).
//!
//! One adaptation covers most of what those micro-animations buy. A Flutter
//! `Transform` scales what is *painted* and leaves the layout box alone, which
//! is exactly why the reference needs an explicit slide to close the gap a
//! shrinking chip leaves behind. Here a chip's spring drives its **advance in
//! the strip** as well as its paint, so its neighbours travel into the space it
//! vacates for free — the same outcome, without the `Flow` delegate. The price
//! is one relayout per animated frame, which the field asks for from its own
//! paint pass.
//!
//! # Keyboard, and the focus gap
//!
//! Up/Down move the panel's highlight, Enter activates it, Escape closes
//! through the host. This is an **addition**: the reference ships no keyboard
//! handling for the dropdown at all. It is also gated on the framework's focus
//! seam — the panel only sees key events once it holds focus, which it claims on
//! a press inside itself, so a caller must complete one pointer interaction with
//! the panel (or its search field) before the arrows do anything. There is no
//! auto-focus-on-appear hook and no focus-traversal seam in the framework, the
//! same gap [`crate::overlay::anchored`] records for Escape.

pub mod field;
pub mod panel;

pub use field::{DropdownFieldView, DropdownFieldWidget, dropdown_field};
pub use panel::{DropdownPanelView, DropdownPanelWidget, dropdown_panel};

use std::rc::Rc;

use frust::authoring::text::{TextContext, TextLayout, TextOverflow, TextStyle};
use frust::authoring::{
    Affine, BezPath, Brush, BuildCtx, ChangeFlags, LayoutCtx, PaintScene, Point, Size, View,
};
use frust::{Color, IconData, SpringDesc, Theme};

use crate::interaction::DISABLED_CONTENT_OPACITY;
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OVERLAY_ANCHOR_GAP, OverlayAlign, OverlayAnchor,
    OverlayPlacement, OverlaySide, anchored_overlay, with_alpha,
};
use crate::tokens::{MaterialDimensions, MaterialSpacing, MaterialSpring};

// ---- Metrics --------------------------------------------------------------
//
// Every value below is the reference's own default. Where one lands exactly on
// a Material spacing/shape token it resolves to that token rather than
// repeating a literal, the same rule `crate::menu`'s metrics follow.
//
// Note which defaults actually render: `M3EDropdownMenuTheme` carries
// `itemOuterRadius: 12` / `itemInnerRadius: 4`, but the item widget reads
// `M3EDropdownItemStyle` instead (`m3e_dropdown_menu_item.dart:84`), whose own
// constructor defaults are `outerRadius: null → 12` and `innerRadius: 6`. The
// style's values are what a stock dropdown paints, so they are what is pinned
// here.

/// Left/right inset from the field's edge to its content
/// (`M3EDropdownFieldStyle.padding`, `horizontal: 16`).
pub const DROPDOWN_FIELD_H_PADDING: f64 = MaterialSpacing::LG;
/// Top/bottom inset of the field's content (`padding`, `vertical: 14`) — the
/// one field metric with no Material spacing token to resolve against.
pub const DROPDOWN_FIELD_V_PADDING: f64 = 14.0;
/// Gap between the field's content and its trailing slot
/// (`m3e_dropdown_menu_field.dart:222`'s `SizedBox(width: 8)`).
pub const DROPDOWN_ICON_GAP: f64 = MaterialSpacing::SM;
/// A trailing/selected icon's side length. The reference reads
/// `M3EThemeData.resolvedIconTheme.size`; 24dp is that theme's own default and
/// the same value [`mod@crate::menu`] pins.
pub const DROPDOWN_ICON_SIZE: f64 = 24.0;
/// Diameter of the field's trailing busy indicator, in logical px
/// (`m3e_dropdown_menu_field.dart:76`'s `SizedBox(width: 18, height: 18)`).
pub const DROPDOWN_FIELD_LOADING_SIZE: f64 = 18.0;
/// Stroke width of that indicator (`strokeWidth: 2`).
pub const DROPDOWN_FIELD_LOADING_STROKE: f64 = 2.0;

/// The panel's height cap, past which it scrolls
/// (`M3EDropdownPanelStyle.maxHeight`).
pub const DROPDOWN_PANEL_MAX_HEIGHT: f64 = 350.0;
/// Gap between the field and the panel (`M3EDropdownPanelStyle.marginTop`, 4) —
/// the same 4dp [`OVERLAY_ANCHOR_GAP`] the anchored host defaults to.
pub const DROPDOWN_PANEL_GAP: f64 = OVERLAY_ANCHOR_GAP;
/// Padding around the panel's item list (`contentPadding: EdgeInsets.all(8)`).
pub const DROPDOWN_CONTENT_PADDING: f64 = MaterialSpacing::SM;
/// Padding around the panel's empty/error message (`EdgeInsets.all(16)`).
pub const DROPDOWN_MESSAGE_PADDING: f64 = MaterialSpacing::LG;
/// Padding around the panel's busy indicator (`EdgeInsets.all(24)`).
pub const DROPDOWN_LOADING_PADDING: f64 = MaterialSpacing::XL;

/// Left/right inset inside one option row
/// (`M3EDropdownItemStyle.itemPadding`, `horizontal: 16`).
pub const DROPDOWN_ITEM_H_PADDING: f64 = MaterialSpacing::LG;
/// Top/bottom inset inside one option row (`itemPadding`, `vertical: 12`).
pub const DROPDOWN_ITEM_V_PADDING: f64 = MaterialSpacing::MD;
/// Vertical gap between two option rows
/// (`m3e_dropdown_menu_panel.dart:177`'s `itemGap ?? 3.0`) — the one row metric
/// with no Material spacing token to resolve against.
pub const DROPDOWN_ITEM_GAP: f64 = 3.0;
/// A row's cap-corner radius (`M3EDropdownItemStyle.outerRadius ?? 12.0`).
pub const DROPDOWN_ITEM_OUTER_RADIUS: f64 = MaterialDimensions::RADIUS_MEDIUM;
/// A row's inner-corner radius at rest (`innerRadius: 6.0`).
pub const DROPDOWN_ITEM_INNER_RADIUS: f64 = 6.0;
/// A row's inner-corner radius while hovered (`hoverRadius: 8.0`).
pub const DROPDOWN_ITEM_HOVER_RADIUS: f64 = MaterialDimensions::RADIUS_SMALL;
/// A row's inner-corner radius while pressed (`pressedRadius: 4.0`).
pub const DROPDOWN_ITEM_PRESSED_RADIUS: f64 = MaterialDimensions::RADIUS_EXTRA_SMALL;

/// Left/right inset inside a selected-option chip
/// (`M3EDropdownChipStyle.padding`, `horizontal: 8`).
pub const DROPDOWN_CHIP_H_PADDING: f64 = MaterialSpacing::SM;
/// Top/bottom inset inside a chip (`padding`, `vertical: 4`).
pub const DROPDOWN_CHIP_V_PADDING: f64 = MaterialSpacing::XS;
/// A chip's corner radius (`borderRadius: BorderRadius.circular(20)`) — a
/// visual constant with no Material shape token at 20dp, and deliberately not
/// [`crate::chips`]' own 8dp (see the module docs' chip divergence).
pub const DROPDOWN_CHIP_RADIUS: f64 = 20.0;
/// Horizontal gap between two chips (`spacing: 6`).
pub const DROPDOWN_CHIP_SPACING: f64 = 6.0;
/// Vertical gap between two chip rows (`runSpacing: 6`).
pub const DROPDOWN_CHIP_RUN_SPACING: f64 = 6.0;
/// Gap between a chip's label and its delete glyph
/// (`m3e_dropdown_chips.dart:186`'s `SizedBox(width: 4)`).
pub const DROPDOWN_CHIP_LABEL_GAP: f64 = MaterialSpacing::XS;
/// A chip's delete-glyph side length. The reference reads the theme's icon size
/// (24dp) here too; this port takes [`crate::chips`]' own 18dp delete-icon size
/// instead, so the two chip families agree on the affordance — the module docs'
/// one deliberate metric deviation.
pub const DROPDOWN_CHIP_ICON_SIZE: f64 = 18.0;

/// The field's placeholder when nothing is selected
/// (`m3e_dropdown_menu_field.dart:167`'s `hintText ?? 'Select'`).
pub const DROPDOWN_HINT: &str = "Select";
/// The panel's message when the filter matches nothing
/// (`M3EDropdownPanelStyle.noItemsFoundText`).
pub const DROPDOWN_EMPTY_TEXT: &str = "No items found";
/// The search field's placeholder (`M3EDropdownSearchStyle.hintText`).
pub const DROPDOWN_SEARCH_HINT: &str = "Search…";

/// The field's hover overlay alpha (`m3e_dropdown_menu_field.dart:272`).
pub const DROPDOWN_FIELD_HOVER_ALPHA: f32 = 0.05;
/// The field's pressed overlay alpha (`m3e_dropdown_menu_field.dart:269`).
pub const DROPDOWN_FIELD_PRESSED_ALPHA: f32 = 0.10;
/// The hint's ink opacity over `onSurface`
/// (`m3e_dropdown_menu_theme.dart:103`).
pub const DROPDOWN_HINT_OPACITY: f32 = 0.6;
/// The empty-state message's ink opacity over `onSurface`
/// (`m3e_dropdown_menu_panel.dart:164`).
pub const DROPDOWN_EMPTY_OPACITY: f32 = 0.5;
/// A disabled row's container opacity over `onSurface`
/// (`m3e_dropdown_menu_item.dart:110`).
pub const DROPDOWN_DISABLED_CONTAINER_OPACITY: f32 = 0.04;

/// The spring both the field's arrow and a chip's scale ride
/// ([`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`], the reference's
/// `M3EMotion.expressiveSpatialDefault` default for `openMotion`/`closeMotion`).
pub(crate) const DROPDOWN_SPRING: SpringDesc = SpringDesc {
    mass: 1.0,
    stiffness: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.stiffness,
    damping_ratio: MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT.damping_ratio,
};

/// The velocity a spring ramp is released with. A spring settles on the target
/// its velocity's *sign* picks, so the magnitude only shapes the launch; this
/// is [`mod@crate::toggle_button`]'s own `FLING_VELOCITY`, kept in step so
/// every spring-morphing control in the catalog launches alike.
pub(crate) const DROPDOWN_FLING_VELOCITY: f64 = 4.0;

/// Progress difference below which a spring ramp counts as settled.
pub(crate) const DROPDOWN_PROGRESS_EPSILON: f64 = 1e-3;

/// The ink every run is *shaped* with; never painted — a run is re-brushed with
/// its resolved per-state color at paint time, and holding the shaping color
/// constant keeps the shape cache from missing on a recolor (the contract
/// [`mod@crate::button`]'s own runs document).
pub(crate) const SHAPING_INK: Color = Color::BLACK;

// ---- Colors (`m3e_dropdown_menu_theme.dart:59-118`) ------------------------

/// Unthemed-fallback `surfaceContainerHighest`.
const FALLBACK_CONTAINER_HIGHEST: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);
/// Unthemed-fallback `surfaceContainerHigh`.
const FALLBACK_CONTAINER_HIGH: Color = Color::from_rgb8(0xEC, 0xE6, 0xF0);
/// Unthemed-fallback `onSurface`.
const FALLBACK_ON_SURFACE: Color = Color::from_rgb8(0x1D, 0x1B, 0x20);
/// Unthemed-fallback `secondaryContainer`.
const FALLBACK_SECONDARY_CONTAINER: Color = Color::from_rgb8(0xE8, 0xDE, 0xF8);
/// Unthemed-fallback `onSecondaryContainer`.
const FALLBACK_ON_SECONDARY_CONTAINER: Color = Color::from_rgb8(0x1D, 0x19, 0x2B);
/// Unthemed-fallback `error`.
const FALLBACK_ERROR: Color = Color::from_rgb8(0xB3, 0x26, 0x1E);

/// One dropdown's resolved color roles (`M3EDropdownMenuTheme`'s ten color
/// methods, plus the two the field and the empty state resolve inline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropdownColors {
    /// The trigger field's fill (`fieldBackgroundColor`).
    pub field_container: Color,
    /// The trigger field's ink (`fieldForegroundColor`), and the ink its
    /// hover/press overlay is drawn from.
    pub field_content: Color,
    /// The panel's fill (`panelBackgroundColor`).
    pub panel_container: Color,
    /// A selected-option chip's fill (`chipBackgroundColor`).
    pub chip_container: Color,
    /// A chip's label and delete-glyph ink (`chipForegroundColor`).
    pub chip_content: Color,
    /// An idle option row's fill (`itemBackgroundColor`).
    pub item_container: Color,
    /// A selected option row's fill (`itemSelectedBackgroundColor`).
    pub item_selected_container: Color,
    /// An idle option row's ink (`itemForegroundColor`).
    pub item_content: Color,
    /// A selected option row's ink (`itemSelectedForegroundColor`).
    pub item_selected_content: Color,
    /// The placeholder's ink (`hintTextStyle`).
    pub hint: Color,
    /// The empty-state message's ink (`m3e_dropdown_menu_panel.dart:164`).
    pub empty: Color,
    /// A load failure's ink (`m3e_dropdown_menu_panel.dart:153`'s
    /// `scheme.error`).
    pub error: Color,
}

impl DropdownColors {
    /// Resolve every role from `theme`, or from the M3 baseline light values
    /// when no theme is threaded into the pass.
    pub fn resolve(theme: Option<&Theme>) -> Self {
        let (
            container_highest,
            container_high,
            on_surface,
            secondary_container,
            on_secondary_container,
            error,
        ) = match theme {
            Some(theme) => {
                let s = theme.scheme();
                (
                    s.surface_container_highest,
                    s.surface_container_high,
                    s.on_surface,
                    s.secondary_container,
                    s.on_secondary_container,
                    s.error,
                )
            }
            None => (
                FALLBACK_CONTAINER_HIGHEST,
                FALLBACK_CONTAINER_HIGH,
                FALLBACK_ON_SURFACE,
                FALLBACK_SECONDARY_CONTAINER,
                FALLBACK_ON_SECONDARY_CONTAINER,
                FALLBACK_ERROR,
            ),
        };
        DropdownColors {
            field_container: container_highest,
            field_content: on_surface,
            panel_container: container_highest,
            chip_container: secondary_container,
            chip_content: on_secondary_container,
            item_container: container_high,
            item_selected_container: secondary_container,
            item_content: on_surface,
            item_selected_content: on_secondary_container,
            hint: with_alpha(on_surface, DROPDOWN_HINT_OPACITY),
            empty: with_alpha(on_surface, DROPDOWN_EMPTY_OPACITY),
            error,
        }
    }

    /// An option row's fill: disabled first, then selected
    /// (`_resolveBackgroundColor`).
    pub fn item_background(&self, enabled: bool, selected: bool) -> Color {
        if !enabled {
            return with_alpha(self.item_content, DROPDOWN_DISABLED_CONTAINER_OPACITY);
        }
        if selected {
            self.item_selected_container
        } else {
            self.item_container
        }
    }

    /// An option row's ink, on the same ladder (`_resolveTextColor`).
    pub fn item_foreground(&self, enabled: bool, selected: bool) -> Color {
        if !enabled {
            return with_alpha(self.item_content, DISABLED_CONTENT_OPACITY);
        }
        if selected {
            self.item_selected_content
        } else {
            self.item_content
        }
    }
}

/// The panel's corner radius, resolved from the theme's shape scale.
///
/// The reference's `containerRadius: 28` lands exactly on the `extraLarge`
/// shape token, so the token is what a themed pass reads — the
/// **explicit > theme > fallback** ladder the workspace's theming conventions
/// require, and the same accessor [`crate::overlay::radius`] already is.
pub(crate) fn container_radius(theme: Option<&Theme>) -> f64 {
    crate::overlay::radius(theme)
}

// ---- The item model -------------------------------------------------------

/// One selectable option (`M3EDropdownItem`).
///
/// The reference's item carries its own `selected` flag and the controller
/// mutates it through `copyWith`. **This port does not**: selection is the
/// app's, handed down through [`DropdownView::selected`]/
/// [`DropdownFieldView::selected`] and requested back through `on_change`, per
/// this catalog's controlled-component rule. `disabled` stays on the item,
/// since it is a property of the option rather than of the selection.
///
/// `value` is the identity a selection is expressed in and must be unique
/// across an item list; `label` is what the field and the panel display and
/// what the filter matches against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropdownItem {
    label: String,
    value: String,
    disabled: bool,
}

/// Build an option labelled `label` carrying `value`.
pub fn dropdown_item(label: impl Into<String>, value: impl Into<String>) -> DropdownItem {
    DropdownItem {
        label: label.into(),
        value: value.into(),
        disabled: false,
    }
}

impl DropdownItem {
    /// Refuse selection of this option (`M3EDropdownItem.disabled`). A disabled
    /// option still renders, dimmed, and still matches the filter.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The displayed text.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The identity a selection is expressed in.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether this option refuses selection.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Whether `item`'s label matches `query` under the reference's filter rule.
///
/// `M3EDropdownController._reapplySearchFilter` lowercases the query once and
/// tests `i.label.toLowerCase().contains(q)` — a **case-insensitive substring
/// match against the label only**, never the value, with an empty query
/// matching everything. Rust's `str::to_lowercase` is the Unicode-aware simple
/// lowercase mapping, the closest equivalent of Dart's `String.toLowerCase`.
pub fn dropdown_matches(item: &DropdownItem, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    item.label.to_lowercase().contains(&query.to_lowercase())
}

/// The subset of `items` [`dropdown_matches`] admits, in the original order.
pub fn dropdown_filter(items: &[DropdownItem], query: &str) -> Vec<DropdownItem> {
    items
        .iter()
        .filter(|item| dropdown_matches(item, query))
        .cloned()
        .collect()
}

/// The selection `values` requests, ordered the way `items` are.
///
/// The reference's `selectedItems` is always `_items.where((i) => i.selected)`
/// — item order, never click order (`m3e_dropdown_controller.dart:83`) — so
/// every reported selection is normalized through this rather than appended to.
/// Values naming no item are dropped, which is what makes a stale selection
/// handed down by an app converge instead of accumulating.
pub(crate) fn ordered_selection(items: &[DropdownItem], values: &[String]) -> Vec<String> {
    items
        .iter()
        .filter(|item| values.iter().any(|v| v == &item.value))
        .map(|item| item.value.clone())
        .collect()
}

// ---- Shared paint helpers -------------------------------------------------

/// A shaped, cached text run, re-brushed at paint. The same lazily-shaped,
/// measured-against-`(style, max_width)` cache [`mod@crate::menu`]'s rows carry.
pub(crate) struct Run {
    pub(crate) content: String,
    layout: Option<TextLayout>,
    /// The `(style, max_width)` the cached `layout` was shaped for.
    shaped_for: Option<(TextStyle, Option<f64>)>,
    /// The `(style, width)` of the last natural (unconstrained) measurement.
    natural: Option<(TextStyle, f64)>,
}

impl Run {
    /// A run holding `content`, unshaped until the first measurement.
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Run {
            content: content.into(),
            layout: None,
            shaped_for: None,
            natural: None,
        }
    }

    /// The run's width with nothing constraining it.
    pub(crate) fn natural_width(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> f64 {
        if let Some((cached, width)) = &self.natural
            && cached == style
        {
            return *width;
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let width = text_ctx.layout(&self.content, style, None).size().width;
        self.natural = Some((style.clone(), width));
        width
    }

    /// Shape (or reuse) the run in `style`, fitted to `max_width`, and return
    /// its measured size.
    ///
    /// A fitted run is a **single ellipsized line**: the reference's field text
    /// is `overflow: TextOverflow.ellipsis` (`m3e_dropdown_menu_field.dart:163`)
    /// and its rows are single-line `Text`s inside an `Expanded`.
    pub(crate) fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: f64) -> Size {
        let key = (style.clone(), Some(max_width));
        if let Some(cached) = &self.layout
            && self.shaped_for.as_ref() == Some(&key)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout_bounded(
            &self.content,
            style,
            Some(max_width.max(0.0) as f32),
            Some(1),
            TextOverflow::Ellipsis,
        );
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_for = Some(key);
        size
    }

    /// The shaped size, or zero for a run no layout pass has reached yet.
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding the [`SHAPING_INK`] it
    /// was shaped with. A never-shaped run paints nothing.
    pub(crate) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        self.paint_transformed(origin, color, Affine::IDENTITY, scene);
    }

    /// Paint the run at `origin` in `color`, with `transform` applied about
    /// `origin` first — how a chip's spring scale reaches its label.
    pub(crate) fn paint_transformed(
        &self,
        origin: Point,
        color: Color,
        transform: Affine,
        scene: &mut dyn PaintScene,
    ) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            if transform != Affine::IDENTITY {
                run.transform = transform * run.transform;
            }
            scene.draw_glyph_run(run);
        }
    }
}

/// An icon slot, parsed once at build into its design-box path — the same
/// record [`mod@crate::menu`]'s rows carry, plus a rotation the field's arrow
/// needs.
pub(crate) struct Glyph {
    path: BezPath,
    design: f64,
    /// The side length to paint at.
    pub(crate) extent: f64,
}

impl Glyph {
    /// Parse `icon`'s path once, to be painted at `extent`.
    pub(crate) fn new(icon: frust::IconSource, extent: f64) -> Self {
        let (path, design) = IconData::from(icon).resolve();
        Glyph {
            path,
            design,
            extent,
        }
    }

    /// The scale factor from the glyph's design box to [`Self::extent`].
    fn scale(&self) -> f64 {
        if self.design > 0.0 {
            self.extent / self.design
        } else {
            1.0
        }
    }

    /// Paint the glyph with its top-left at `origin`, in `color`.
    pub(crate) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let scaled = Affine::scale(self.scale()) * self.path.clone();
        scene.fill_path(origin, &scaled, &Brush::Solid(color));
    }

    /// Paint the glyph rotated `radians` about its own center — the field
    /// arrow's `Transform.rotate(angle: _arrowCtrl.value)`
    /// (`m3e_dropdown_menu_field.dart:93`).
    pub(crate) fn paint_rotated(
        &self,
        origin: Point,
        color: Color,
        radians: f64,
        scene: &mut dyn PaintScene,
    ) {
        let half = self.extent / 2.0;
        let pivot = kurbo::Vec2::new(half, half);
        let transform = Affine::translate(pivot)
            * Affine::rotate(radians)
            * Affine::translate(-pivot)
            * Affine::scale(self.scale());
        scene.fill_path(
            origin,
            &(transform * self.path.clone()),
            &Brush::Solid(color),
        );
    }
}

// ---- The composite dropdown ----------------------------------------------

/// A view-held, typed selection callback.
pub(crate) type OnSelection<State> = Rc<dyn Fn(&mut State, Vec<String>)>;
/// A view-held, typed open-request callback.
pub(crate) type OnOpen<State> = Rc<dyn Fn(&mut State, bool)>;
/// A view-held, typed query-request callback.
pub(crate) type OnQuery<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative dropdown: a [`dropdown_panel`] inside
/// [`crate::overlay::anchored`]. See [`dropdown`].
///
/// The host is composed fresh in each pass from the props below rather than
/// retained: [`AnchoredOverlayView`] type-erases its content at construction,
/// so there is no way to reach back into the panel and re-set one of its own
/// props once it is inside — the same shape [`crate::menu::MenuView`] takes.
pub struct DropdownView<State: 'static> {
    items: Vec<DropdownItem>,
    selected: Vec<String>,
    multi: bool,
    open: bool,
    query: String,
    searchable: bool,
    loading: bool,
    error: Option<String>,
    empty_text: String,
    max_selections: usize,
    max_height: f64,
    placement: OverlayPlacement,
    anchor: OverlayAnchor,
    on_change: OnSelection<State>,
    on_open: Option<OnOpen<State>>,
    on_query: Option<OnQuery<State>>,
}

/// Build the panel half of a dropdown over `items`, to be mounted as the top
/// child of a full-area [`frust::Stack`] (or a transparent navigator page) and
/// pointed at the same [`OverlayAnchor`] the [`dropdown_field`] captures into.
///
/// `on_change(state, values)` reports the selection an activation *requests*,
/// already ordered the way `items` are (item order, never click order); the panel never
/// mutates its own [`DropdownView::selected`].
///
/// The default placement is the reference's `expandDirection: auto` below the
/// field, leading edges flush, at [`DROPDOWN_PANEL_GAP`] — with the anchored
/// host's own collision flip standing in for `auto`'s space check.
pub fn dropdown<State: 'static, F: Fn(&mut State, Vec<String>) + 'static>(
    items: Vec<DropdownItem>,
    on_change: F,
) -> DropdownView<State> {
    DropdownView {
        items,
        selected: Vec::new(),
        multi: false,
        open: true,
        query: String::new(),
        searchable: false,
        loading: false,
        error: None,
        empty_text: DROPDOWN_EMPTY_TEXT.to_string(),
        max_selections: 0,
        max_height: DROPDOWN_PANEL_MAX_HEIGHT,
        placement: OverlayPlacement::default()
            .align(OverlayAlign::Start)
            .offset(DROPDOWN_PANEL_GAP),
        anchor: OverlayAnchor::new(),
        on_change: Rc::new(on_change),
        on_open: None,
        on_query: None,
    }
}

impl<State: 'static> DropdownView<State> {
    /// Anchor the panel to the rect `anchor` carries — the same cell the
    /// [`dropdown_field`] writes, which is also what width-matches the panel to
    /// the field.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Hand the panel the app-confirmed selection (option values).
    pub fn selected(mut self, selected: Vec<String>) -> Self {
        self.selected = selected;
        self
    }

    /// Allow more than one option at a time (`singleSelect: false`). The
    /// default here is **single**-select, inverting the reference's own default
    /// so the simpler mode is the one a caller gets for free.
    pub fn multi(mut self, multi: bool) -> Self {
        self.multi = multi;
        self
    }

    /// Cap a multi-select at `max` options (`maxSelections`); `0` is unlimited.
    /// Deselecting is never capped.
    pub fn max_selections(mut self, max: usize) -> Self {
        self.max_selections = max;
        self
    }

    /// Hand a **kept-mounted** panel the app's open flag, so closing it plays
    /// the host's exit ramp instead of vanishing.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Show the search field above the option list (`searchEnabled`).
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }

    /// Hand the panel the app-confirmed filter query. Only rendered when
    /// [`searchable`](Self::searchable) is set; the filter itself applies
    /// regardless, so an app may drive it from its own field.
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    /// Set the query callback: each edit of the search field reports the text
    /// it *requests* (`onSearchChanged`).
    pub fn on_query<F: Fn(&mut State, String) + 'static>(mut self, on_query: F) -> Self {
        self.on_query = Some(Rc::new(on_query));
        self
    }

    /// Show the reference's centered busy row instead of the option list — the
    /// panel half of the async poll seam (see the [module docs](self)).
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Show a load failure instead of the option list
    /// (`m3e_dropdown_menu_panel.dart:148`'s `_errorMessage`).
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    /// Replace the empty-state message (`noItemsFoundText`).
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// Cap the panel's height, past which it scrolls (`maxHeight`).
    pub fn max_height(mut self, max_height: f64) -> Self {
        self.max_height = max_height;
        self
    }

    /// Set the side the panel opens on (`bottom` by default). The reference's
    /// `expandDirection` `up`/`down` map onto this; its `auto` is the anchored
    /// host's own collision flip, which is on by default.
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self
    }

    /// Set the cross-axis alignment (`start` by default, the edge the
    /// reference's `CompositedTransformFollower` pins).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self
    }

    /// Set the gap between field and panel (`marginTop`, 4dp by default).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self
    }

    /// Set the open callback. Fired with `false` by a light dismiss, by Escape
    /// once the host holds focus, and by a single-select activation — the
    /// reference's `_selectSingleItem` closes the dropdown after picking.
    pub fn on_open<F: Fn(&mut State, bool) + 'static>(mut self, on_open: F) -> Self {
        self.on_open = Some(Rc::new(on_open));
        self
    }

    /// Compose the host and its panel from the current props.
    fn host(&self) -> AnchoredOverlayView<State> {
        let on_change = self.on_change.clone();
        let mut panel = dropdown_panel(self.items.clone(), move |state: &mut State, values| {
            on_change(state, values)
        })
        .anchor(&self.anchor)
        .selected(self.selected.clone())
        .multi(self.multi)
        .max_selections(self.max_selections)
        .query(self.query.clone())
        .searchable(self.searchable)
        .loading(self.loading)
        .error(self.error.clone())
        .empty_text(self.empty_text.clone())
        .max_height(self.max_height)
        .open(self.open);
        if let Some(on_open) = self.on_open.clone() {
            panel = panel.on_open(move |state: &mut State, open| on_open(state, open));
        }
        if let Some(on_query) = self.on_query.clone() {
            panel = panel.on_query(move |state: &mut State, query| on_query(state, query));
        }
        let mut host = anchored_overlay(panel)
            .anchor(&self.anchor)
            .placement(self.placement)
            .open(self.open);
        if let Some(on_open) = self.on_open.clone() {
            host = host.on_dismiss(move |state: &mut State| on_open(state, false));
        }
        host
    }
}

impl<State: 'static> View<State> for DropdownView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.host(), ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.host(), &prev.host(), element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.host(), element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Brightness;

    fn fruit() -> Vec<DropdownItem> {
        vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana"),
            dropdown_item("Blackcurrant", "blackcurrant").disabled(true),
        ]
    }

    #[test]
    fn the_metrics_are_the_reference_s_own_defaults() {
        assert_eq!(DROPDOWN_FIELD_H_PADDING, 16.0);
        assert_eq!(DROPDOWN_FIELD_V_PADDING, 14.0);
        assert_eq!(DROPDOWN_ICON_GAP, 8.0);
        assert_eq!(DROPDOWN_ICON_SIZE, 24.0);
        assert_eq!(DROPDOWN_FIELD_LOADING_SIZE, 18.0);
        assert_eq!(DROPDOWN_FIELD_LOADING_STROKE, 2.0);
        assert_eq!(DROPDOWN_PANEL_MAX_HEIGHT, 350.0);
        assert_eq!(DROPDOWN_PANEL_GAP, 4.0);
        assert_eq!(DROPDOWN_CONTENT_PADDING, 8.0);
        assert_eq!(DROPDOWN_MESSAGE_PADDING, 16.0);
        assert_eq!(DROPDOWN_LOADING_PADDING, 24.0);
        assert_eq!(DROPDOWN_ITEM_H_PADDING, 16.0);
        assert_eq!(DROPDOWN_ITEM_V_PADDING, 12.0);
        assert_eq!(DROPDOWN_ITEM_GAP, 3.0);
        // The *style*'s defaults, not the theme's unread 12/4 pair.
        assert_eq!(DROPDOWN_ITEM_OUTER_RADIUS, 12.0);
        assert_eq!(DROPDOWN_ITEM_INNER_RADIUS, 6.0);
        assert_eq!(DROPDOWN_ITEM_HOVER_RADIUS, 8.0);
        assert_eq!(DROPDOWN_ITEM_PRESSED_RADIUS, 4.0);
        assert_eq!(DROPDOWN_CHIP_H_PADDING, 8.0);
        assert_eq!(DROPDOWN_CHIP_V_PADDING, 4.0);
        assert_eq!(DROPDOWN_CHIP_RADIUS, 20.0);
        assert_eq!(DROPDOWN_CHIP_SPACING, 6.0);
        assert_eq!(DROPDOWN_CHIP_RUN_SPACING, 6.0);
        assert_eq!(DROPDOWN_CHIP_LABEL_GAP, 4.0);
        assert_eq!(DROPDOWN_HINT, "Select");
        assert_eq!(DROPDOWN_EMPTY_TEXT, "No items found");
    }

    #[test]
    fn the_container_radius_is_the_theme_s_extra_large_shape_token() {
        let theme = crate::baseline();
        assert_eq!(container_radius(Some(&theme)), theme.shape.extra_large);
        // The reference's own `containerRadius: 28` default.
        assert_eq!(container_radius(None), 28.0);
    }

    #[test]
    fn every_color_folds_onto_its_material_role() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let s = theme.scheme();
        let c = DropdownColors::resolve(Some(&theme));
        assert_eq!(c.field_container, s.surface_container_highest);
        assert_eq!(c.field_content, s.on_surface);
        assert_eq!(c.panel_container, s.surface_container_highest);
        assert_eq!(c.chip_container, s.secondary_container);
        assert_eq!(c.chip_content, s.on_secondary_container);
        assert_eq!(c.item_container, s.surface_container_high);
        assert_eq!(c.item_selected_container, s.secondary_container);
        assert_eq!(c.item_content, s.on_surface);
        assert_eq!(c.item_selected_content, s.on_secondary_container);
        assert_eq!(c.hint, with_alpha(s.on_surface, DROPDOWN_HINT_OPACITY));
        assert_eq!(c.empty, with_alpha(s.on_surface, DROPDOWN_EMPTY_OPACITY));
        assert_eq!(c.error, s.error);
    }

    #[test]
    fn the_unthemed_fallbacks_are_the_m3_baseline_light_values() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        assert_eq!(
            DropdownColors::resolve(None),
            DropdownColors::resolve(Some(&light))
        );
    }

    #[test]
    fn the_colors_follow_a_live_brightness_flip() {
        let light = crate::baseline().with_brightness(Brightness::Light);
        let dark = crate::baseline().with_brightness(Brightness::Dark);
        assert_ne!(
            DropdownColors::resolve(Some(&light)).panel_container,
            DropdownColors::resolve(Some(&dark)).panel_container
        );
    }

    #[test]
    fn a_non_material_theme_still_resolves_every_role() {
        // An app is free to thread a bare theme; nothing here may panic.
        let bare = Theme::neutral();
        let _ = DropdownColors::resolve(Some(&bare));
        let _ = container_radius(Some(&bare));
    }

    #[test]
    fn the_item_ladder_is_disabled_then_selected() {
        let c = DropdownColors::resolve(None);
        assert_eq!(
            c.item_background(false, true),
            with_alpha(c.item_content, DROPDOWN_DISABLED_CONTAINER_OPACITY),
            "disabled outranks selected"
        );
        assert_eq!(
            c.item_foreground(false, true),
            with_alpha(c.item_content, DISABLED_CONTENT_OPACITY)
        );
        assert_eq!(c.item_background(true, true), c.item_selected_container);
        assert_eq!(c.item_background(true, false), c.item_container);
        assert_eq!(c.item_foreground(true, true), c.item_selected_content);
        assert_eq!(c.item_foreground(true, false), c.item_content);
    }

    // ---- the filter -------------------------------------------------------

    #[test]
    fn an_empty_query_admits_every_item() {
        let items = fruit();
        assert_eq!(dropdown_filter(&items, "").len(), 3);
    }

    #[test]
    fn the_filter_is_a_case_insensitive_substring_of_the_label() {
        let items = fruit();
        // Not an anchored prefix: `nan` matches inside `Banana`.
        let narrowed = dropdown_filter(&items, "nan");
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].label(), "Banana");
        // Case-folded on both sides.
        assert_eq!(dropdown_filter(&items, "APPLE").len(), 1);
        assert_eq!(dropdown_filter(&items, "apple").len(), 1);
        // Two matches keep their original order.
        let b = dropdown_filter(&items, "b");
        assert_eq!(
            b.iter().map(DropdownItem::label).collect::<Vec<_>>(),
            ["Banana", "Blackcurrant"]
        );
    }

    #[test]
    fn the_filter_never_matches_a_value_only_the_label() {
        let items = vec![dropdown_item("Apple", "pomme")];
        assert!(dropdown_filter(&items, "pomme").is_empty());
        assert_eq!(dropdown_filter(&items, "app").len(), 1);
    }

    #[test]
    fn a_disabled_item_still_matches_the_filter() {
        let items = fruit();
        let narrowed = dropdown_filter(&items, "black");
        assert_eq!(narrowed.len(), 1);
        assert!(narrowed[0].is_disabled());
    }

    // ---- the composite, end to end ---------------------------------------

    /// Field + host mounted the documented way — the trigger in the app's own
    /// layout, the panel as the top child of a full-area [`frust::Stack`],
    /// both pointed at one [`OverlayAnchor`] — with the app owning every
    /// controlled value, exactly as a real consumer does.
    mod composite {
        use super::super::*;
        use frust::FrameTime;
        use frust::authoring::{
            InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton, PointerEvent,
            PointerPhase, Size, any, text::TextContext,
        };
        use frust_core::RenderRoot;
        use std::any::Any;

        const WINDOW: Size = Size::new(320.0, 480.0);

        #[derive(Default)]
        struct AppState {
            open: bool,
            selected: Vec<String>,
            query: String,
        }

        fn items() -> Vec<DropdownItem> {
            vec![
                dropdown_item("Apple", "apple"),
                dropdown_item("Banana", "banana"),
                dropdown_item("Cherry", "cherry"),
            ]
        }

        /// The app's own tree: the trigger in normal layout, the panel host as
        /// the stack's topmost child, one anchor shared by both.
        fn tree(
            anchor: &OverlayAnchor,
            multi: bool,
            searchable: bool,
            state: &AppState,
        ) -> frust::StackView<AppState> {
            let selected = state.selected.clone();
            let open = state.open;
            let query = state.query.clone();
            frust::Stack(vec![
                any(dropdown_field::<AppState>(items())
                    .anchor(anchor)
                    .multi(multi)
                    .selected(selected.clone())
                    .open(open)
                    .on_open(|s: &mut AppState, next| s.open = next)
                    .on_change(|s: &mut AppState, values| s.selected = values)),
                any(
                    dropdown(items(), |s: &mut AppState, values| s.selected = values)
                        .anchor(anchor)
                        .multi(multi)
                        .selected(selected)
                        .open(open)
                        .searchable(searchable)
                        .query(query)
                        .on_query(|s: &mut AppState, text| s.query = text)
                        .on_open(|s: &mut AppState, next| s.open = next),
                ),
            ])
        }

        struct Harness {
            root: RenderRoot<AppState, frust::StackView<AppState>>,
            state: AppState,
            tcx: TextContext,
            anchor: OverlayAnchor,
            multi: bool,
            searchable: bool,
        }

        impl Harness {
            fn new(multi: bool) -> Self {
                Harness::with(multi, false)
            }

            fn with(multi: bool, searchable: bool) -> Self {
                let mut h = Harness {
                    root: RenderRoot::new(),
                    state: AppState::default(),
                    tcx: TextContext::new(),
                    anchor: OverlayAnchor::new(),
                    multi,
                    searchable,
                };
                h.pass();
                h
            }

            /// One rebuild + layout + paint pass, the way a shell's frame runs
            /// it — the paint is what captures the trigger's rect into the
            /// anchor and advances every ramp.
            fn pass(&mut self) {
                let anchor = self.anchor.clone();
                let multi = self.multi;
                let searchable = self.searchable;
                let mut logic = move |state: &mut AppState| tree(&anchor, multi, searchable, state);
                self.root.rebuild(&mut logic, &mut self.state);
                self.root
                    .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
                self.paint();
            }

            fn paint(&mut self) {
                struct Sink;
                impl frust::authoring::PaintScene for Sink {
                    fn fill_rect(
                        &mut self,
                        _origin: Point,
                        _size: Size,
                        _color: frust::authoring::Color,
                    ) {
                    }
                    fn draw_text(&mut self, _origin: Point, _text: &str) {}
                }
                self.root.paint(&mut Sink, FrameTime::ZERO);
            }

            fn pointer(&mut self, phase: PointerPhase, x: f64, y: f64) {
                self.root.event(
                    &mut self.state,
                    &InputEvent::Pointer(PointerEvent {
                        phase,
                        position: Point::new(x, y),
                        button: PointerButton::Primary,
                    }),
                );
            }

            fn click(&mut self, x: f64, y: f64) {
                self.pointer(PointerPhase::Down, x, y);
                self.pointer(PointerPhase::Up, x, y);
            }

            fn key(&mut self, named: NamedKey) {
                self.send(Key::Named(named));
            }

            fn typed(&mut self, text: &str) {
                self.send(Key::Character(text.to_string()));
            }

            fn send(&mut self, key: Key) {
                self.root.event(
                    &mut self.state,
                    &InputEvent::Key(KeyEvent {
                        key,
                        modifiers: Modifiers::default(),
                        repeat: false,
                    }),
                );
            }

            /// The trigger's captured rect — the field is the stack's first
            /// child, so it sits at the window origin.
            fn field(&self) -> frust::authoring::Rect {
                self.anchor.rect()
            }

            /// The window-space y of option row `i`. A row is its label's line
            /// box plus the item padding, floored at the check glyph's own
            /// extent, so the check-sized band is the safe centre to aim at.
            fn row_y(&self, i: usize) -> f64 {
                let band = 2.0 * DROPDOWN_ITEM_V_PADDING + DROPDOWN_ICON_SIZE;
                self.list_top() + i as f64 * (band + DROPDOWN_ITEM_GAP) + band / 2.0
            }

            /// The window-space top of the panel's own box.
            fn panel_top(&self) -> f64 {
                self.field().y1 + DROPDOWN_PANEL_GAP
            }

            /// The window-space top of the scrollable option list, past the
            /// search header when one is mounted.
            fn list_top(&self) -> f64 {
                let header = if self.searchable {
                    panel::SEARCH_MARGIN_TOP
                        + panel::SEARCH_FIELD_HEIGHT
                        + panel::SEARCH_MARGIN_BOTTOM
                } else {
                    0.0
                };
                self.panel_top() + header + DROPDOWN_CONTENT_PADDING
            }

            /// A point inside the mounted search field.
            fn search_point(&self) -> Point {
                Point::new(
                    panel::SEARCH_MARGIN_H + 20.0,
                    self.panel_top() + panel::SEARCH_MARGIN_TOP + panel::SEARCH_FIELD_HEIGHT / 2.0,
                )
            }
        }

        #[test]
        fn open_then_pick_then_close_is_one_round_trip_through_the_app_s_own_state() {
            let mut h = Harness::new(false);
            assert!(!h.state.open);
            assert_eq!(h.field().width(), WINDOW.width, "the trigger is captured");

            // 1. Open: the press lands on the trigger, since the closed host is
            //    input-transparent.
            let field = h.field();
            h.click(40.0, field.center().y);
            assert!(h.state.open, "the field requested the open state");
            h.pass();

            // 2. Pick: the press now lands on the panel, which the host places
            //    below the trigger.
            let y = h.row_y(1);
            h.click(40.0, y);
            assert_eq!(h.state.selected, vec!["banana".to_string()]);
            assert!(!h.state.open, "a single-select pick closes the dropdown");
            h.pass();

            // 3. The trigger toggles back open from its picked state.
            let center = h.field().center().y;
            h.click(40.0, center);
            assert!(h.state.open);
        }

        #[test]
        fn a_press_outside_the_panel_light_dismisses_through_the_host() {
            let mut h = Harness::new(false);
            let field = h.field();
            h.click(40.0, field.center().y);
            h.pass();
            assert!(h.state.open);

            // Far below the panel: the host's own light dismiss.
            h.pointer(PointerPhase::Down, 10.0, WINDOW.height - 10.0);
            assert!(!h.state.open);
            assert!(h.state.selected.is_empty());
        }

        #[test]
        fn the_arrows_reach_the_panel_once_a_press_inside_it_has_claimed_focus() {
            let mut h = Harness::new(false);
            let field = h.field();
            h.click(40.0, field.center().y);
            h.pass();

            // A press on the panel's own padding claims focus without
            // activating anything.
            let panel_top = h.panel_top();
            h.click(4.0, panel_top + 2.0);
            assert!(h.state.selected.is_empty());
            assert!(h.state.open, "a background press never dismisses");

            h.key(NamedKey::ArrowDown);
            h.key(NamedKey::ArrowDown);
            h.key(NamedKey::Enter);
            assert_eq!(h.state.selected, vec!["banana".to_string()]);
            assert!(!h.state.open);
        }

        #[test]
        fn escape_dismisses_through_the_host_once_it_holds_focus() {
            let mut h = Harness::new(false);
            let field = h.field();
            h.click(40.0, field.center().y);
            h.pass();

            let panel_top = h.panel_top();
            h.click(4.0, panel_top + 2.0);
            h.key(NamedKey::Escape);
            assert!(!h.state.open);
        }

        #[test]
        fn typing_in_the_search_field_narrows_the_options_it_offers() {
            let mut h = Harness::with(false, true);
            let field = h.field();
            h.click(40.0, field.center().y);
            h.pass();

            // Focus the header field, then type: the panel forwards every key
            // it does not navigate with to the focused editable, which reports
            // the query the app then feeds back down.
            let at = h.search_point();
            h.click(at.x, at.y);
            h.typed("b");
            assert_eq!(h.state.query, "b");
            h.pass();

            // `Banana` is the only label containing a `b`, so it is now the
            // list's first (and only) row.
            let y = h.row_y(0);
            h.click(40.0, y);
            assert_eq!(h.state.selected, vec!["banana".to_string()]);
        }

        #[test]
        fn a_multi_select_round_trip_keeps_the_panel_open_and_grows_the_field() {
            let mut h = Harness::new(true);
            let field = h.field();
            h.click(40.0, field.center().y);
            h.pass();

            let first = h.row_y(0);
            h.click(40.0, first);
            assert_eq!(h.state.selected, vec!["apple".to_string()]);
            assert!(h.state.open, "a multi-select pick keeps the panel open");
            h.pass();

            let third = h.row_y(2);
            h.click(40.0, third);
            assert_eq!(
                h.state.selected,
                vec!["apple".to_string(), "cherry".to_string()],
                "item order, not click order"
            );
            // Settle the chips' entrance springs before measuring the growth.
            for _ in 0..120 {
                h.pass();
            }
            assert!(
                h.field().height() > field.height(),
                "two chips grew the trigger past its hint-only height ({} vs {})",
                h.field().height(),
                field.height()
            );
        }
    }

    #[test]
    fn a_selection_is_normalized_into_item_order_and_drops_unknown_values() {
        let items = fruit();
        let requested = vec![
            "banana".to_string(),
            "nope".to_string(),
            "apple".to_string(),
        ];
        assert_eq!(
            ordered_selection(&items, &requested),
            vec!["apple".to_string(), "banana".to_string()],
            "item order wins over the order the values arrived in"
        );
        assert!(ordered_selection(&items, &[]).is_empty());
    }
}
