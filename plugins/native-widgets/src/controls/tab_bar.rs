//! `TabBar` — a real, **bare** `UITabBar` built and driven from Rust on
//! iOS/iPadOS. **iOS-arm-only**: the first control in
//! [`super::IOS_ONLY_KINDS`] — neither the macOS nor the Android arm registers
//! it.
//!
//! # A bare bar, never a `UITabBarController` (decision D5)
//!
//! frust owns screen ownership and routing (`frust::Router`); a
//! `UITabBarController` would own both — its own child view controllers, its
//! own selection-drives-navigation behaviour. So this control is the bar and
//! nothing else: a tap reports the *requested* tab, the app decides whether to
//! route (`RouteNavigator::go`) and feeds the confirmed id back as props. The
//! bar never navigates by itself.
//!
//! # No macOS arm, no Android arm (decision D2)
//!
//! macOS has no bottom-tab-bar idiom (`NSTabView` is a different control with
//! a different job), and Android's `BottomNavigationView` is Material — this
//! plugin never assumes an AndroidX/Material dependency the app did not add
//! (`segmented.rs`'s *No Android arm*). So this module carries one
//! `#[cfg(target_os = "ios")] mod platform` and nothing else, and the builder
//! (`crate::api::builders::NativeTabBarView`) renders the frust-drawn refusal
//! banner **at compile time** on every other target, naming both facts.
//!
//! # The items ride flat params; icon bytes ride a side table
//!
//! `crate::runtime::Params` reads flat objects only, so each item crosses the
//! wire as `tab<i><Field>` keys ([`item_key`]) under an [`ITEM_COUNT`], capped
//! at [`MAX_ITEMS`] on decode: its title, enabled flag, optional badge, and an
//! optional SF Symbol name per icon slot. **Encoded icon bytes never enter the
//! params** — the same reasoning `crate::controls::image` gives for images: the
//! builder publishes them into this module's per-slot side table
//! ([`publish_icon_bytes`]), which answers a **revision** the params carry as
//! [`ICONS_REV`] so a bytes change is visible to the differ, and
//! [`TabBarProps::decode`] reads the bytes back from the table (the table is
//! the authority, so a replayed create shows the current icons). The table
//! compares **by content**, not `Arc` identity: an app that rebuilds its
//! `Vec<TabItem>` every frame from `include_bytes!` would otherwise bump the
//! revision every rebuild. The entry is removed exactly once, on component
//! teardown ([`retire_icon_bytes`], registered by the builder's
//! `Component::init`), which is what bounds the table.
//!
//! # The controlled-component contract — `Segmented`'s, id-valued at the api
//!
//! On the wire the selection is an **index** into the items (the builder
//! resolves the app's `TabId` to its index before encoding, and maps the
//! reported index back to that item's `TabId` in its callback), so this
//! module is `Segmented`'s shape verbatim:
//!
//! 1. **Write-back.** [`TabBarProps::plan`] takes the index the platform last
//!    reported (`observed`) and re-plans [`Setter::SelectedTab`] when it
//!    disagrees with the app's value — a rejected tap snaps back on the next
//!    differing params (the same v1 identical-props limitation `switch.rs`
//!    documents applies).
//! 2. **Echo guard: none needed.** `UITabBar.selectedItem`'s setter does not
//!    call the delegate; should anything ever re-enter,
//!    `crate::runtime::with_runtime`'s re-entrancy drop catches it.
//!
//! The item array is rebuilt **only** when the items changed
//! ([`Setter::TabItems`]); otherwise the plan only moves `selectedItem`. A
//! rebuild always re-plans the selection, because the new array holds new
//! `UITabBarItem` objects the old `selectedItem` does not name.
//!
//! # Select vs reselect
//!
//! `UITabBarDelegate.tabBar:didSelectItem:` fires for every tap on an enabled
//! item, including the one already showing. The one target class
//! (`crate::apple::events`) remembers the tab the **app last confirmed** —
//! only a [`Setter::SelectedTab`] apply updates it (`note_tab_showing`), a
//! tap itself never does — and classifies each tap with [`tap_kind`]: the
//! app-confirmed tab tapped again is
//! [`crate::events::EVENT_KIND_RESELECTED`] (an app's "scroll to top / pop to
//! root"), anything else [`crate::events::EVENT_KIND_SELECTION`] — including
//! a retap of a tab the app never confirmed (no write-back), even though
//! UIKit has already moved `selectedItem` and the highlight to it either way
//! (the same v1 identical-props limitation the *Write-back* section above
//! names; the classification is independent of that highlight). Each
//! `UITabBarItem` is built with its index as its `tag`, which is what the
//! delegate reports.
//!
//! # Theme
//!
//! `crate::api::theme` folds `accent_ink` into [`super::TINT`]
//! (`UITabBar.tintColor`, the selected item), `muted` into [`UNSELECTED_TINT`]
//! (`unselectedItemTintColor`) and `surface_bg` into
//! [`super::BACKGROUND_COLOR`] — applied through a `UITabBarAppearance`
//! installed as BOTH `standardAppearance` and `scrollEdgeAppearance`, so iOS
//! 15+ never renders the transparent scroll-edge variant a bare bar would
//! otherwise get. `create` installs a default-background appearance up front
//! for the same reason, themed or not.
//!
//! # Height and the home indicator
//!
//! The bar is [`BAR_HEIGHT`] points tall plus the window's bottom safe-area
//! inset; the builder's inset-aware wrapper sizes the slot at layout time and
//! the `UITabBar` frame covers the whole slot, so its background runs under
//! the home indicator while UIKit keeps the items above it through the bar's
//! own safe-area layout (a device-gate observable).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use super::{BACKGROUND_COLOR, Plan, Setter, TINT, color, owned_text, slot_of};
use crate::NativeWidgetError;
use crate::events::{EVENT_KIND_RESELECTED, EVENT_KIND_SELECTION, EventPayload, unpack_index};
use crate::registry::SlotId;
use crate::runtime::{NativeEvent, Params};

/// The registered kind string the api layer injects as `__frustControl`.
pub(crate) const KIND: &str = "tab_bar";

/// `"itemCount"` — how many `tab<i>…` item groups follow.
pub(crate) const ITEM_COUNT: &str = "itemCount";
/// `"selected"` — the app-owned selected item index (controlled). Absent
/// when the app's `TabId` names no item.
pub(crate) const SELECTED: &str = "selected";
/// `"iconsRev"` — the icon-bytes side table's revision for this slot (module
/// doc's *icon bytes ride a side table*); `0` when no item has byte icons.
pub(crate) const ICONS_REV: &str = "iconsRev";
/// `"unselectedTint"` — packed ARGB, `null`-able:
/// `UITabBar.unselectedItemTintColor`.
pub(crate) const UNSELECTED_TINT: &str = "unselectedTint";

/// Per-item field suffixes — see [`item_key`].
pub(crate) const FIELD_TITLE: &str = "Title";
/// The badge text (absent: no badge).
pub(crate) const FIELD_BADGE: &str = "Badge";
/// Whether the item is tappable (absent: `true`).
pub(crate) const FIELD_ENABLED: &str = "Enabled";
/// The unselected icon's SF Symbol name (absent: bytes or no icon).
pub(crate) const FIELD_SYMBOL: &str = "Symbol";
/// The selected icon's SF Symbol name (absent: bytes, or the unselected one).
pub(crate) const FIELD_SELECTED_SYMBOL: &str = "SelectedSymbol";

/// The most items one bar decodes — a guard against a malformed
/// `itemCount`, far above anything a tab bar can show (UIKit folds more
/// than five into nothing on a bare bar; there is no "More" tab without a
/// `UITabBarController`).
pub(crate) const MAX_ITEMS: usize = 16;

/// The bar's own height in points, above the bottom safe-area inset —
/// UIKit's compact tab bar height on iPhone and on iPadOS's bottom bar.
pub(crate) const BAR_HEIGHT: f64 = 49.0;

/// The point box a byte icon is normalized into ([`icon_decode_scale`]) —
/// Apple's regular tab-bar glyph size.
pub(crate) const ICON_POINTS: f64 = 25.0;

/// The wire key of field `field` of item `index`: `"tab0Title"`, …
pub(crate) fn item_key(index: usize, field: &str) -> String {
    format!("tab{index}{field}")
}

/// The `UIImage` scale to decode a byte icon of `width`×`height` pixels at,
/// so it renders inside an [`ICON_POINTS`] box: `1.0` for an image already
/// that small (never upscaled), else its longest side over the box — a 75px
/// icon decodes at scale 3 (25pt, crisp on a 3x screen). A non-finite or
/// non-positive size decodes at `1.0`.
pub(crate) fn icon_decode_scale(width: f64, height: f64) -> f64 {
    let longest = width.max(height);
    if !longest.is_finite() || longest <= ICON_POINTS {
        1.0
    } else {
        longest / ICON_POINTS
    }
}

/// Classify a tap on item `tapped` against `showing` — the tab the app last
/// **confirmed** selected, not merely the one UIKit is highlighting (module
/// doc's *Select vs reselect*): the confirmed tab tapped again is a
/// reselect, anything else — including a retap of a tab the app never
/// confirmed — a selection. `showing` moves only on a `Setter::SelectedTab`
/// apply, never on a tap itself, which is what keeps a rejected-then-retapped
/// tap classified as a selection. Called by the one target class's
/// `tabBar:didSelectItem:` — pure so the rule is host-tested.
pub(crate) fn tap_kind(showing: Option<usize>, tapped: usize) -> i32 {
    if showing == Some(tapped) {
        EVENT_KIND_RESELECTED
    } else {
        EVENT_KIND_SELECTION
    }
}

// --- the icon-bytes side table ----------------------------------------------

/// One item's published icon bytes — `None` for an icon slot that is an SF
/// Symbol or absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ItemIconBytes {
    /// The unselected (always-shown) icon.
    pub(crate) icon: Option<Arc<[u8]>>,
    /// The selected-state icon.
    pub(crate) selected_icon: Option<Arc<[u8]>>,
}

impl ItemIconBytes {
    fn is_empty(&self) -> bool {
        self.icon.is_none() && self.selected_icon.is_none()
    }
}

/// A slot's published icon bytes and the revision they were published at.
struct Published {
    rev: u64,
    items: Arc<[ItemIconBytes]>,
}

thread_local! {
    /// Main-thread confined, like `crate::controls::image`'s publish table:
    /// the builder publishes during a main-thread rebuild and the factory
    /// decodes during the main-thread post-frame poll.
    static ICON_BYTES: RefCell<HashMap<SlotId, Published>> = RefCell::new(HashMap::new());
    /// The next revision any slot publishes at — process-monotonic on this
    /// thread, so a revision never repeats for a slot.
    static NEXT_REV: Cell<u64> = const { Cell::new(1) };
}

/// Publish `items`' icon bytes for `slot`, answering the revision the params
/// carry as [`ICONS_REV`]: the same content published again answers the same
/// revision, different content a fresh one, and a bar with no byte icons at
/// all publishes nothing and answers `0` (removing any earlier entry).
pub(crate) fn publish_icon_bytes(slot: SlotId, items: Vec<ItemIconBytes>) -> u64 {
    ICON_BYTES.with(|table| {
        let mut table = table.borrow_mut();
        if items.iter().all(ItemIconBytes::is_empty) {
            table.remove(&slot);
            return 0;
        }
        if let Some(published) = table.get(&slot)
            && *published.items == *items
        {
            return published.rev;
        }
        let rev = NEXT_REV.with(|next| {
            let rev = next.get();
            next.set(rev + 1);
            rev
        });
        table.insert(
            slot,
            Published {
                rev,
                items: items.into(),
            },
        );
        rev
    })
}

/// The icon bytes currently published for `slot`, if any.
pub(crate) fn icon_bytes(slot: SlotId) -> Option<Arc<[ItemIconBytes]>> {
    ICON_BYTES.with(|table| {
        table
            .borrow()
            .get(&slot)
            .map(|published| Arc::clone(&published.items))
    })
}

/// Drop `slot`'s entry — the builder's `on_cleanup`, exactly once per mounted
/// component (module doc). Idempotent.
pub(crate) fn retire_icon_bytes(slot: SlotId) {
    ICON_BYTES.with(|table| {
        table.borrow_mut().remove(&slot);
    });
}

// --- props ------------------------------------------------------------------

/// Where one icon comes from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IconSource {
    /// An SF Symbol name (`UIImage.systemImageNamed:`).
    Symbol(String),
    /// Encoded image bytes (`UIImage.imageWithData:scale:`).
    Bytes(Arc<[u8]>),
}

/// One decoded tab item.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabItemProps {
    /// `UIBarItem.title`.
    pub(crate) title: String,
    /// The always-shown icon (`UIBarItem.image`), `None` for none.
    pub(crate) icon: Option<IconSource>,
    /// `UITabBarItem.selectedImage`, `None` for "tint the unselected one".
    pub(crate) selected_icon: Option<IconSource>,
    /// `UITabBarItem.badgeValue`, `None` for no badge.
    pub(crate) badge: Option<String>,
    /// `UIBarItem.enabled`.
    pub(crate) enabled: bool,
}

/// The marker type registered under [`KIND`] — by the iOS arm only.
pub(crate) struct TabBar;

/// Everything a `TabBar` slot can be told, as one Rust-diffed value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabBarProps {
    /// The differ's slot id — **not a property**; `create`/`decode` need it
    /// for the target and the icon side table.
    pub(crate) slot: SlotId,
    /// The items, in order.
    pub(crate) items: Vec<TabItemProps>,
    /// The app-owned selected item (controlled), `None` for none. Always
    /// `< items.len()` when `Some` (normalized on decode).
    pub(crate) selected: Option<usize>,
    /// Packed ARGB selected-item tint (`UITabBar.tintColor`), or `None`.
    pub(crate) tint: Option<i32>,
    /// Packed ARGB unselected-item tint, or `None`.
    pub(crate) unselected_tint: Option<i32>,
    /// Packed ARGB bar background (through `UITabBarAppearance`), or `None`
    /// for the platform's default (blurred) material.
    pub(crate) background: Option<i32>,
}

impl TabBarProps {
    /// The state a freshly constructed bar is in once `create` has installed
    /// its default appearance: no items, nothing selected, platform colours.
    pub(crate) fn platform_default(slot: SlotId) -> Self {
        Self {
            slot,
            items: Vec::new(),
            selected: None,
            tint: None,
            unselected_tint: None,
            background: None,
        }
    }

    /// Decode a `TabBar` slot's params, reading byte icons from the side
    /// table (module doc). A byte icon wins over a symbol name for the same
    /// slot; a missing title decodes empty (degrade, never a dead slot).
    ///
    /// # Errors
    /// [`NativeWidgetError::Params`] when the reserved identity keys are
    /// missing.
    pub(crate) fn decode(params: &Params<'_>) -> Result<Self, NativeWidgetError> {
        let slot = slot_of(params)?;
        let count = params
            .int(ITEM_COUNT)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0)
            .min(MAX_ITEMS);
        let bytes = icon_bytes(slot);
        let items: Vec<TabItemProps> = (0..count)
            .map(|index| {
                let published = bytes.as_ref().and_then(|all| all.get(index));
                let symbol = |field: &str| {
                    owned_text(params, &item_key(index, field))
                        .filter(|name| !name.is_empty())
                        .map(IconSource::Symbol)
                };
                TabItemProps {
                    title: owned_text(params, &item_key(index, FIELD_TITLE)).unwrap_or_default(),
                    icon: published
                        .and_then(|b| b.icon.clone())
                        .map(IconSource::Bytes)
                        .or_else(|| symbol(FIELD_SYMBOL)),
                    selected_icon: published
                        .and_then(|b| b.selected_icon.clone())
                        .map(IconSource::Bytes)
                        .or_else(|| symbol(FIELD_SELECTED_SYMBOL)),
                    badge: owned_text(params, &item_key(index, FIELD_BADGE)),
                    enabled: params.flag(&item_key(index, FIELD_ENABLED)).unwrap_or(true),
                }
            })
            .collect();
        let selected = params
            .int(SELECTED)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|&index| index < items.len());
        Ok(Self {
            slot,
            items,
            selected,
            tint: color(params, TINT),
            unselected_tint: color(params, UNSELECTED_TINT),
            background: color(params, BACKGROUND_COLOR),
        })
    }

    /// The setter-call plan for `old` → `new`, given the index the platform
    /// last reported (`observed`, `None` until the user has tapped).
    ///
    /// Order is load-bearing: the item array first (only when it changed),
    /// then the selection — re-planned whenever the array was rebuilt, the
    /// app's value changed, or the platform drifted from it — then the three
    /// colours.
    pub(crate) fn plan<'a>(old: &Self, new: &'a Self, observed: Option<usize>) -> Plan<'a> {
        let mut plan = Plan::new();
        let items_changed = old.items != new.items;
        if items_changed {
            plan.push(Setter::TabItems(&new.items));
        }
        let drifted = observed.is_some_and(|platform| Some(platform) != new.selected);
        if items_changed || old.selected != new.selected || drifted {
            plan.push(Setter::SelectedTab(new.selected));
        }
        if old.tint != new.tint {
            plan.push(Setter::TabBarTint(new.tint));
        }
        if old.unselected_tint != new.unselected_tint {
            plan.push(Setter::TabBarUnselectedTint(new.unselected_tint));
        }
        if old.background != new.background {
            plan.push(Setter::TabBarBackground(new.background));
        }
        plan
    }
}

/// Decode an [`EVENT_KIND_SELECTION`]/[`EVENT_KIND_RESELECTED`] firing
/// against a bar currently holding `item_count` items: records the
/// write-back drift signal (`observed`) and decodes to
/// [`EventPayload::Selected`]/[`EventPayload::Reselected`] with the item's
/// index (the builder's callback maps it back to the item's `TabId`).
///
/// `None` for any other kind (defensive), for a negative index, or for an
/// index past the live items (a report racing an item-array rebuild) — no
/// requested tab to deliver, and `observed` is left untouched.
pub(crate) fn decode_event(
    observed: &mut Option<usize>,
    item_count: usize,
    event: NativeEvent,
) -> Option<EventPayload> {
    let reselect = match event.kind {
        EVENT_KIND_SELECTION => false,
        EVENT_KIND_RESELECTED => true,
        _ => return None,
    };
    let index = unpack_index(event.detail).filter(|&index| index < item_count)?;
    *observed = Some(index);
    Some(if reselect {
        EventPayload::Reselected(index)
    } else {
        EventPayload::Selected(index)
    })
}

#[cfg(target_os = "ios")]
pub(crate) mod platform {
    //! The iOS half: build a bare `UITabBar` and apply the shared plan.
    //!
    //! # Construction-time normalization
    //!
    //! `create` installs a `UITabBarAppearance` configured with the default
    //! (blurred material) background as both `standardAppearance` and
    //! `scrollEdgeAppearance` before the diffed create plan runs — the
    //! "platform default" [`TabBarProps::platform_default`] names, with the
    //! transparent scroll-edge variant a bare bar would otherwise get on iOS
    //! 15+ ruled out. [`Setter::TabBarBackground`] re-installs the same pair
    //! (opaque with the colour, or default again for `None`).
    //!
    //! # The delegate is the target
    //!
    //! Taps arrive through `UITabBarDelegate.tabBar:didSelectItem:` on the one
    //! target class ([`FrustNativeControlTarget`]), set as the bar's
    //! (weak) delegate after the create plan and retained here; the target
    //! also remembers the tab the app last confirmed, for [`super::tap_kind`]
    //! — every [`Setter::SelectedTab`] apply tells it, a tap never does.

    use std::sync::Once;

    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSArray, NSData, NSString};
    use objc2_ui_kit::{UIImage, UITabBar, UITabBarAppearance, UITabBarItem};

    use super::{
        IconSource, KIND, TabBar, TabBarProps, TabItemProps, decode_event, icon_decode_scale,
    };
    use crate::NativeWidgetError;
    use crate::apple::{FrustNativeControlTarget, NativeCtx, NativeView};
    use crate::controls::platform;
    use crate::controls::{Plan, Setter};
    use crate::events::EventPayload;
    use crate::runtime::{NativeEvent, NativeWidget, Params};

    /// A live tab bar's retained state — `SegmentedState`'s shape.
    pub(crate) struct TabBarState {
        view: Retained<UITabBar>,
        /// The bar's delegate. `UITabBar.delegate` is weak, so this field is
        /// its only retain (`crate::apple::events`' *Target retention*).
        target: Retained<FrustNativeControlTarget>,
        /// The index the platform last reported, `None` while untouched —
        /// [`TabBarProps::plan`]'s write-back drift signal.
        observed: Option<usize>,
        /// How many items the bar holds — bounds [`decode_event`].
        item_count: usize,
    }

    impl NativeWidget for TabBar {
        type Props = TabBarProps;
        type State = TabBarState;

        fn decode_props(params: &Params<'_>) -> Result<Self::Props, NativeWidgetError> {
            TabBarProps::decode(params)
        }

        fn create(
            ctx: &mut NativeCtx<'_, '_>,
            props: &Self::Props,
        ) -> Result<(NativeView, Self::State), NativeWidgetError> {
            let mtm = ctx.mtm();
            let view = UITabBar::new(mtm);
            // Module doc's *Construction-time normalization*.
            install_appearance(mtm, &view, None);
            let target = FrustNativeControlTarget::tab_bar_target(mtm, props.slot);
            let plan = TabBarProps::plan(&TabBarProps::platform_default(props.slot), props, None);
            apply_all(mtm, &view, &target, &plan);
            // Wired after the initial plan, the other controls' order (a
            // programmatic `selectedItem` never calls the delegate anyway).
            target.attach_tab_bar(&view);
            // `UITabBar` is a direct `UIView` subclass (no `UIControl` hop).
            let handle = NativeView::new(Retained::clone(&view).into_super(), mtm);
            Ok((
                handle,
                TabBarState {
                    view,
                    target,
                    observed: None,
                    item_count: props.items.len(),
                },
            ))
        }

        fn update(
            ctx: &mut NativeCtx<'_, '_>,
            state: &mut Self::State,
            old: &Self::Props,
            new: &Self::Props,
        ) -> Result<(), NativeWidgetError> {
            let plan = TabBarProps::plan(old, new, state.observed);
            apply_all(ctx.mtm(), &state.view, &state.target, &plan);
            state.item_count = new.items.len();
            // Nothing on this arm can fail — cleared unconditionally, as on
            // every other controlled control's iOS arm.
            state.observed = None;
            Ok(())
        }

        fn on_event(state: &mut Self::State, event: NativeEvent) -> Option<EventPayload> {
            decode_event(&mut state.observed, state.item_count, event)
        }

        fn dispose(
            _ctx: &mut NativeCtx<'_, '_>,
            state: Self::State,
        ) -> Result<(), NativeWidgetError> {
            // Clear the weak delegate so a stray in-flight tap can't reach a
            // torn-down slot; dropping `state` releases the target.
            state.target.detach_tab_bar(&state.view);
            Ok(())
        }
    }

    /// Execute a whole [`Plan`], front to back.
    fn apply_all(
        mtm: MainThreadMarker,
        view: &UITabBar,
        target: &FrustNativeControlTarget,
        plan: &Plan<'_>,
    ) {
        for setter in plan {
            apply(mtm, view, target, setter);
        }
    }

    /// Execute one planned property write against `view`.
    fn apply(
        mtm: MainThreadMarker,
        view: &UITabBar,
        target: &FrustNativeControlTarget,
        setter: &Setter<'_>,
    ) {
        match *setter {
            Setter::TabItems(items) => {
                let built: Vec<Retained<UITabBarItem>> = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| build_item(mtm, index, item))
                    .collect();
                view.setItems_animated(Some(&NSArray::from_retained_slice(&built)), false);
            }
            // The delegate is not called for a programmatic selection (module
            // doc's echo guard), so the snap-back is a plain write.
            Setter::SelectedTab(selected) => {
                let item = selected.and_then(|index| {
                    view.items()
                        .filter(|items| index < items.count())
                        .map(|items| items.objectAtIndex(index))
                });
                view.setSelectedItem(item.as_deref());
                target.note_tab_showing(item.and(selected));
            }
            Setter::TabBarTint(argb) => {
                let color = platform::optional_ui_color(argb);
                // SAFETY: objc2 marks `setTintColor:` unsafe only because the
                // header leaves the argument's nullability unannotated; nil is
                // documented to inherit the superview's tint — exactly
                // `TabBarTint(None)`'s "platform colour" meaning
                // (`crate::controls::platform::set_image_tint`'s same note).
                unsafe { view.setTintColor(color.as_deref()) };
            }
            Setter::TabBarUnselectedTint(argb) => {
                view.setUnselectedItemTintColor(platform::optional_ui_color(argb).as_deref());
            }
            Setter::TabBarBackground(argb) => install_appearance(mtm, view, argb),
            ref other => platform::warn_unexpected_setter(KIND, other),
        }
    }

    /// Install one appearance as both `standardAppearance` and
    /// `scrollEdgeAppearance` (module doc's *Construction-time
    /// normalization*): opaque with `background` when set, the default
    /// blurred material when not.
    fn install_appearance(mtm: MainThreadMarker, view: &UITabBar, background: Option<i32>) {
        let appearance = UITabBarAppearance::init(UITabBarAppearance::alloc(mtm));
        match background {
            Some(argb) => {
                appearance.configureWithOpaqueBackground();
                appearance.setBackgroundColor(Some(&platform::ui_color(argb)));
            }
            None => appearance.configureWithDefaultBackground(),
        }
        view.setStandardAppearance(&appearance);
        view.setScrollEdgeAppearance(Some(&appearance));
    }

    /// One `UITabBarItem`, tagged with its index (what the delegate reports).
    fn build_item(
        mtm: MainThreadMarker,
        index: usize,
        item: &TabItemProps,
    ) -> Retained<UITabBarItem> {
        let title = NSString::from_str(&item.title);
        let image = item.icon.as_ref().and_then(ui_image);
        // `index < MAX_ITEMS`, so it always fits an `NSInteger`.
        let built = UITabBarItem::initWithTitle_image_tag(
            UITabBarItem::alloc(mtm),
            Some(&title),
            image.as_deref(),
            index as isize,
        );
        if let Some(selected) = item.selected_icon.as_ref().and_then(ui_image) {
            built.setSelectedImage(Some(&selected));
        }
        let badge = item.badge.as_deref().map(NSString::from_str);
        built.setBadgeValue(badge.as_deref());
        built.setEnabled(item.enabled);
        built
    }

    /// The `UIImage` for one icon source, `None` (logged once) when an SF
    /// Symbol name is unknown or the bytes do not decode — the item then shows
    /// its title alone (degrade, never a dead slot).
    fn ui_image(source: &IconSource) -> Option<Retained<UIImage>> {
        let image = match source {
            IconSource::Symbol(name) => UIImage::systemImageNamed(&NSString::from_str(name)),
            IconSource::Bytes(bytes) => {
                let data = NSData::with_bytes(bytes);
                UIImage::imageWithData(&data).and_then(|probe| {
                    // SAFETY: objc2 marks `size` unsafe only for thread
                    // safety ("might not be thread-safe"); this runs on the
                    // main thread against an image this function alone owns.
                    let size = unsafe { probe.size() };
                    let scale = icon_decode_scale(size.width, size.height);
                    if scale == 1.0 {
                        Some(probe)
                    } else {
                        UIImage::imageWithData_scale(&data, scale)
                    }
                })
            }
        };
        if image.is_none() {
            warn_icon_unavailable_once();
        }
        image
    }

    /// One-time warning for an icon that produced no image.
    fn warn_icon_unavailable_once() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            log::warn!(
                "frust-native-widgets: a native_tab_bar icon produced no image (an unknown SF \
                 Symbol name, or bytes UIImage cannot decode) — the item shows its title only"
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Tier;
    use crate::events::pack_index;
    use crate::runtime::with_identity;

    fn decode_at(slot: SlotId, body: &str) -> TabBarProps {
        let raw = with_identity(KIND, slot, body);
        TabBarProps::decode(&Params::new(&raw)).expect("decodes")
    }

    fn decode(body: &str) -> TabBarProps {
        decode_at(5, body)
    }

    const THREE: &str = "\"itemCount\":3,\"tab0Title\":\"Home\",\"tab0Symbol\":\"house\",\
         \"tab1Title\":\"Search\",\"tab1Symbol\":\"magnifyingglass\",\"tab1Badge\":\"3\",\
         \"tab2Title\":\"Profile\",\"tab2Enabled\":false";

    #[test]
    fn absent_fields_decode_to_the_platform_defaults_and_plan_nothing() {
        let props = decode("");
        assert_eq!(props, TabBarProps::platform_default(5));
        assert!(TabBarProps::plan(&TabBarProps::platform_default(5), &props, None).is_empty());
    }

    #[test]
    fn items_decode_from_the_flat_count_and_indexed_keys() {
        let props = decode(&format!("{THREE},\"selected\":1"));
        assert_eq!(props.items.len(), 3);
        assert_eq!(props.items[0].title, "Home");
        assert_eq!(
            props.items[0].icon,
            Some(IconSource::Symbol("house".into()))
        );
        assert_eq!(props.items[0].selected_icon, None);
        assert_eq!(props.items[1].badge.as_deref(), Some("3"));
        assert!(props.items[1].enabled);
        assert!(!props.items[2].enabled);
        assert_eq!(props.items[2].icon, None);
        assert_eq!(props.selected, Some(1));
    }

    #[test]
    fn the_count_is_capped_and_an_out_of_range_selection_is_none() {
        assert_eq!(decode("\"itemCount\":100000").items.len(), MAX_ITEMS);
        assert!(decode("\"itemCount\":-2").items.is_empty());
        assert_eq!(decode(&format!("{THREE},\"selected\":3")).selected, None);
        assert_eq!(decode(&format!("{THREE},\"selected\":-1")).selected, None);
        assert_eq!(decode("\"selected\":0").selected, None);
    }

    #[test]
    fn an_empty_symbol_name_is_no_icon() {
        let props = decode("\"itemCount\":1,\"tab0Symbol\":\"\"");
        assert_eq!(props.items[0].icon, None);
    }

    #[test]
    fn the_create_plan_is_items_then_selection_then_colours() {
        let props = decode(&format!(
            "{THREE},\"selected\":2,\"tint\":1,\"unselectedTint\":2,\"backgroundColor\":3"
        ));
        assert_eq!(
            TabBarProps::plan(&TabBarProps::platform_default(5), &props, None),
            vec![
                Setter::TabItems(&props.items),
                Setter::SelectedTab(Some(2)),
                Setter::TabBarTint(Some(1)),
                Setter::TabBarUnselectedTint(Some(2)),
                Setter::TabBarBackground(Some(3)),
            ]
        );
    }

    #[test]
    fn a_selection_change_alone_moves_selected_item_without_rebuilding_items() {
        let old = decode(&format!("{THREE},\"selected\":0"));
        let new = decode(&format!("{THREE},\"selected\":1"));
        assert_eq!(
            TabBarProps::plan(&old, &new, None),
            vec![Setter::SelectedTab(Some(1))]
        );
        assert!(
            TabBarProps::plan(&new, &new, None).is_empty(),
            "unchanged props plan nothing"
        );
    }

    #[test]
    fn a_badge_change_rebuilds_the_items_and_reasserts_the_selection() {
        let old = decode(&format!("{THREE},\"selected\":1"));
        let new = decode(&format!("{THREE},\"selected\":1,\"tab0Badge\":\"new\""));
        assert_eq!(
            TabBarProps::plan(&old, &new, None),
            vec![Setter::TabItems(&new.items), Setter::SelectedTab(Some(1))],
            "a rebuilt array holds new UITabBarItems the old selectedItem does not name"
        );
    }

    #[test]
    fn a_platform_drift_is_written_back() {
        let old = decode(&format!("{THREE},\"selected\":0"));
        let new = decode(&format!("{THREE},\"selected\":0,\"tint\":4"));
        assert_eq!(
            TabBarProps::plan(&old, &new, Some(2)),
            vec![Setter::SelectedTab(Some(0)), Setter::TabBarTint(Some(4))]
        );
        assert_eq!(
            TabBarProps::plan(&old, &new, Some(0)),
            vec![Setter::TabBarTint(Some(4))],
            "an observed value matching the app's is no drift"
        );
    }

    #[test]
    fn clearing_a_colour_plans_the_nullable_setter() {
        let themed = decode("\"tint\":1,\"unselectedTint\":2,\"backgroundColor\":3");
        let plain = decode("");
        assert_eq!(
            TabBarProps::plan(&themed, &plain, None),
            vec![
                Setter::TabBarTint(None),
                Setter::TabBarUnselectedTint(None),
                Setter::TabBarBackground(None),
            ]
        );
    }

    #[test]
    fn every_planned_setter_reports_its_documented_tier() {
        let props = decode(&format!(
            "{THREE},\"selected\":1,\"tint\":1,\"unselectedTint\":2,\"backgroundColor\":3"
        ));
        for setter in TabBarProps::plan(&TabBarProps::platform_default(5), &props, None) {
            let expected = if matches!(setter, Setter::TabItems(_)) {
                Tier::Relayout
            } else {
                Tier::Cheap
            };
            assert_eq!(setter.tier(), expected, "{setter:?}");
        }
    }

    // --- the icon-bytes side table ----------------------------------------

    fn bytes(content: &[u8]) -> Arc<[u8]> {
        Arc::from(content.to_vec().into_boxed_slice())
    }

    #[test]
    fn byte_icons_decode_from_the_side_table_and_win_over_symbols() {
        let slot = 9_001;
        let rev = publish_icon_bytes(
            slot,
            vec![
                ItemIconBytes {
                    icon: Some(bytes(&[1, 2])),
                    selected_icon: Some(bytes(&[3])),
                },
                ItemIconBytes::default(),
            ],
        );
        assert!(rev > 0);
        let props = decode_at(
            slot,
            &format!(
                "\"itemCount\":2,\"iconsRev\":{rev},\"tab0Symbol\":\"house\",\
                 \"tab1Symbol\":\"gear\""
            ),
        );
        assert_eq!(props.items[0].icon, Some(IconSource::Bytes(bytes(&[1, 2]))));
        assert_eq!(
            props.items[0].selected_icon,
            Some(IconSource::Bytes(bytes(&[3])))
        );
        assert_eq!(props.items[1].icon, Some(IconSource::Symbol("gear".into())));
        retire_icon_bytes(slot);
        assert!(icon_bytes(slot).is_none());
    }

    #[test]
    fn the_same_content_republishes_at_the_same_revision() {
        let slot = 9_002;
        let first = publish_icon_bytes(
            slot,
            vec![ItemIconBytes {
                icon: Some(bytes(&[7, 7])),
                selected_icon: None,
            }],
        );
        // A different `Arc`, same bytes — the app rebuilt its items.
        let again = publish_icon_bytes(
            slot,
            vec![ItemIconBytes {
                icon: Some(bytes(&[7, 7])),
                selected_icon: None,
            }],
        );
        assert_eq!(first, again, "content-equal icons keep their revision");
        let changed = publish_icon_bytes(
            slot,
            vec![ItemIconBytes {
                icon: Some(bytes(&[8])),
                selected_icon: None,
            }],
        );
        assert_ne!(changed, first);
        retire_icon_bytes(slot);
    }

    #[test]
    fn a_bar_with_no_byte_icons_publishes_nothing() {
        let slot = 9_003;
        publish_icon_bytes(
            slot,
            vec![ItemIconBytes {
                icon: Some(bytes(&[1])),
                selected_icon: None,
            }],
        );
        assert_eq!(
            publish_icon_bytes(slot, vec![ItemIconBytes::default(); 3]),
            0
        );
        assert!(
            icon_bytes(slot).is_none(),
            "a symbol-only bar holds no table entry"
        );
    }

    #[test]
    fn byte_icons_normalize_into_the_point_box() {
        assert_eq!(icon_decode_scale(25.0, 25.0), 1.0);
        assert_eq!(icon_decode_scale(16.0, 10.0), 1.0, "never upscaled");
        assert_eq!(icon_decode_scale(75.0, 75.0), 3.0);
        assert_eq!(
            icon_decode_scale(50.0, 100.0),
            4.0,
            "the longest side decides"
        );
        assert_eq!(icon_decode_scale(f64::NAN, 10.0), 1.0);
        assert_eq!(icon_decode_scale(0.0, 0.0), 1.0);
    }

    // --- events -----------------------------------------------------------

    #[test]
    fn a_tap_on_another_item_is_a_selection_and_on_the_showing_one_a_reselect() {
        assert_eq!(tap_kind(Some(0), 1), EVENT_KIND_SELECTION);
        assert_eq!(tap_kind(None, 0), EVENT_KIND_SELECTION);
        assert_eq!(tap_kind(Some(2), 2), EVENT_KIND_RESELECTED);
    }

    #[test]
    fn a_tab_the_app_rejects_stays_a_selection_when_retapped_not_a_reselect() {
        // The app never confirms the first tap (no `Setter::SelectedTab`
        // write-back), so `showing` — the tab the app last confirmed — stays
        // put across both taps: a tap itself never moves it (module doc's
        // *Select vs reselect*). Retapping the same rejected tab must
        // classify as another selection, not a reselect, even though
        // UIKit's own highlight has already moved to it either way.
        let confirmed = Some(0);
        assert_eq!(tap_kind(confirmed, 1), EVENT_KIND_SELECTION);
        assert_eq!(tap_kind(confirmed, 1), EVENT_KIND_SELECTION);
    }

    fn event(kind: i32, index: isize) -> NativeEvent {
        NativeEvent {
            kind,
            detail: pack_index(index),
        }
    }

    #[test]
    fn selection_and_reselect_decode_and_update_the_drift_signal() {
        let mut observed = None;
        assert_eq!(
            decode_event(&mut observed, 3, event(EVENT_KIND_SELECTION, 2)),
            Some(EventPayload::Selected(2))
        );
        assert_eq!(observed, Some(2));
        assert_eq!(
            decode_event(&mut observed, 3, event(EVENT_KIND_RESELECTED, 2)),
            Some(EventPayload::Reselected(2))
        );
        assert_eq!(observed, Some(2));
    }

    #[test]
    fn an_index_past_the_live_items_or_negative_decodes_to_nothing() {
        let mut observed = Some(1);
        assert_eq!(
            decode_event(&mut observed, 3, event(EVENT_KIND_SELECTION, 3)),
            None
        );
        assert_eq!(
            decode_event(&mut observed, 3, event(EVENT_KIND_RESELECTED, -1)),
            None
        );
        assert_eq!(observed, Some(1), "left untouched");
    }

    #[test]
    fn a_misrouted_kind_decodes_to_nothing() {
        let mut observed = None;
        assert_eq!(
            decode_event(&mut observed, 3, event(crate::events::EVENT_KIND_CLICK, 0)),
            None
        );
        assert_eq!(observed, None);
    }
}
