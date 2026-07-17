//! Baseline widget set: Text, Button, Image, Column/Row, Stack, ScrollView, etc.
//! (spec §6.4).
//!
//! Ships the [`text`] leaf plus the spec §6.2 primitive layout containers —
//! [`Row`]/[`Column`] ([`FlexView`]), [`Stack`], [`Padding`], [`Align`], and
//! [`SizedBox`] — built as `View`/`Widget` pairs over `forgekit-core`'s
//! [`AnyView`](forgekit_core::AnyView)/[`ChildPod`](forgekit_core::ChildPod)
//! substrate. Containers own their children directly as `ChildPod`s (the arena
//! stays single-root); see [`forgekit_core::widget::ChildPod`] for the rationale.
//!
//! # Shared child plumbing
//!
//! The container modules build/rebuild/teardown their heterogeneous children
//! through the crate-private [`build_child`]/[`rebuild_child`]/[`teardown_child`]
//! helpers and route pointer events through [`route_event`] (multi-child
//! containers — `Flex`/`Stack`) or [`route_event_single`] (one-child wrappers —
//! `Padding`/`Align`/`SizedBox`). Each child is an
//! [`AnyView`](forgekit_core::AnyView) whose element (`Box<dyn Widget>`) is stored
//! double-boxed inside a `ChildPod`, so a later rebuild can recover
//! `&mut Box<dyn Widget>` to drive `AnyView`'s type-erased reconciliation.

mod align;
mod button;
mod checkbox;
pub mod cupertino;
mod flex;
mod gesture;
mod image;
pub mod material;
pub mod nav;
mod padding;
mod scroll;
mod sized;
mod slider;
mod stack;
mod text;
mod textinput;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use forgekit_core::{
    AnyView, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, PointerButton,
    PointerEvent, PointerPhase, View, Widget,
};
use kurbo::Point;

pub use align::{Align, AlignView, AlignWidget, Alignment};
pub use button::{Button, ButtonView, ButtonWidget, button};
pub use checkbox::{Checkbox, CheckboxView, CheckboxWidget, checkbox};
pub use flex::{
    Axis, Column, CrossAxisAlignment, FlexChild, FlexView, FlexWidget, MainAxisAlignment, Row,
    flexible, inflexible, keyed,
};
pub use gesture::{GestureDetector, GestureDetectorView, GestureDetectorWidget};
pub use image::{Image, ImageError, ImageFit, ImageSource, ImageView, ImageWidget};
pub use nav::navigator::{
    NavigatorController, NavigatorView, NavigatorWidget, PageBuilder, PopResult, ResultCallback,
    navigator,
};
pub use nav::path::{Location, PathPattern, RouteParams};
pub use nav::router::{
    DEFAULT_REDIRECT_LIMIT, ErrorBuilder, Redirect, Resolution, ResolvedPage, Route, RouteBuilder,
    Router,
};
pub use nav::transition::{PageTransition, Timing, TransitionSpec};
pub use padding::{EdgeInsets, Padding, PaddingView, PaddingWidget};
pub use scroll::{ScrollView, ScrollWidget, scroll_view};
pub use sized::{SizedBox, SizedBoxView, SizedBoxWidget};
pub use slider::{Slider, SliderView, SliderWidget, slider};
pub use stack::{Stack, StackView, StackWidget};
pub use text::{TextView, TextWidget, text};
pub use textinput::{TextInput, TextInputView, TextInputWidget, text_input};

// ---------------------------------------------------------------------
// Material 3 Expressive widget catalog (Phase 6c, task 14) — flat
// re-exports so app code (via the `forgekit` facade) never has to name
// `forgekit_widgets::material::*` directly, mirroring the baseline
// widgets' flat re-export shape above.
// ---------------------------------------------------------------------
pub use material::appbar::{AppBar, AppBarView, AppBarWidget, app_bar};
pub use material::button_group::{ButtonGroup, ButtonGroupView, ButtonGroupWidget, button_group};
pub use material::card::{
    CardVariant, CardView, CardWidget, card, elevated_card, filled_card, outlined_card,
};
pub use material::chips::{
    AssistChip, AssistChipView, AssistChipWidget, FilterChip, FilterChipView, FilterChipWidget,
    assist_chip, filter_chip,
};
pub use material::dialog::{DialogView, DialogWidget, dialog, show_dialog};
pub use material::fab::{FabSize, FabView, FabWidget, extended_fab, fab};
pub use material::fab_menu::{
    FabMenu, FabMenuItem, FabMenuView, FabMenuWidget, fab_menu, fab_menu_item,
};
pub use material::list_item::{
    ListItem, ListItemLines, ListItemWidget, ONE_LINE_HEIGHT, THREE_LINE_HEIGHT, TWO_LINE_HEIGHT,
    list_item,
};
pub use material::list_view::{ListView, ListViewWidget, list_view};
pub use material::loading_indicator::{
    LoadingIndicator, LoadingIndicatorView, LoadingIndicatorWidget, loading_indicator,
};
pub use material::navbar::{
    NavItem, NavigationBar, NavigationBarView, NavigationBarWidget, nav_item, navigation_bar,
};
pub use material::progress::{
    CircularProgress, CircularProgressView, CircularProgressWidget, LinearProgress,
    LinearProgressView, LinearProgressWidget, ProgressValue, circular_progress, linear_progress,
};
pub use material::shape_morph::{RoundedPolygon, morph_path};
pub use material::sheet::{BottomSheetView, BottomSheetWidget, bottom_sheet, show_bottom_sheet};
pub use material::split_button::{SplitButton, SplitButtonView, SplitButtonWidget, split_button};
pub use material::switch::{Switch, SwitchView, SwitchWidget, switch};
pub use material::toolbar::{
    DockedToolbar, FloatingToolbar, ToolbarVariant, ToolbarView, ToolbarWidget, docked_toolbar,
    floating_toolbar,
};

// ---------------------------------------------------------------------
// Cupertino (iOS) widget catalog (Phase 6c, task 14) — same flat
// re-export rationale as the Material block above.
// ---------------------------------------------------------------------
pub use cupertino::action_sheet::{
    CupertinoActionSheetView, CupertinoActionSheetWidget, show_action_sheet,
};
pub use cupertino::activity_indicator::{
    CupertinoActivityIndicator, CupertinoActivityIndicatorView, CupertinoActivityIndicatorWidget,
    cupertino_activity_indicator,
};
pub use cupertino::alert_dialog::{
    CupertinoActionStyle, CupertinoAlertDialogView, CupertinoAlertDialogWidget,
    CupertinoDialogAction, action, show_cupertino_alert,
};
pub use cupertino::button::{
    CupertinoButton, CupertinoButtonSize, CupertinoButtonStyle, CupertinoButtonView,
    CupertinoButtonWidget, cupertino_button,
};
pub use cupertino::navbar::{
    CupertinoNavBar, CupertinoNavBarView, CupertinoNavBarWidget, cupertino_nav_bar,
};
pub use cupertino::switch::{
    CupertinoSwitch, CupertinoSwitchView, CupertinoSwitchWidget, cupertino_switch,
};
pub use cupertino::tabbar::{
    CupertinoTabBar, CupertinoTabBarView, CupertinoTabBarWidget, TabItem, cupertino_tab_bar,
    tab_item,
};

/// A widget-held, `State`-erased app-state callback adapter (see
/// [`erase_callback`]).
pub(crate) type ErasedCallback = Box<dyn FnMut(&mut EventCtx)>;

/// A widget-held, `State`-erased callback adapter carrying one value argument
/// (see [`erase_callback_arg`]).
pub(crate) type ErasedArgCallback<A> = Box<dyn FnMut(&mut EventCtx, A)>;

/// A view-held, typed callback carrying one value argument (Checkbox's `bool`,
/// Slider's `f64`), erased to [`ErasedArgCallback`] on build.
pub(crate) type TypedArgCallback<State, A> = std::rc::Rc<dyn Fn(&mut State, A)>;

/// A stable identity for a list child, so a container's reconciliation can match
/// a child to its live widget *by key* across reorders/inserts instead of by
/// position — the difference between "the third row's widget" and "row #42's
/// widget" when the list is shuffled (spec §6.3).
///
/// Built from any [`Hash`] value (an item id, a string name, an index) via the
/// `From` impls below and [`keyed`](crate::keyed); the hashed `u64` is what the
/// reconciler compares. Two children in the same list must not collide — a
/// duplicate key is a `debug_assert` tripwire that falls back to positional
/// reconciliation (see [`rebuild_children`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ChildKey(u64);

impl ChildKey {
    /// Hash any [`Hash`] value into a `ChildKey`. Backs the `From` impls and
    /// [`keyed`](crate::keyed)'s `impl Into<ChildKey>` argument.
    pub fn new(value: impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hasher);
        ChildKey(hasher.finish())
    }
}

macro_rules! child_key_from {
    ($($t:ty),* $(,)?) => {
        $(
            impl From<$t> for ChildKey {
                fn from(value: $t) -> Self {
                    ChildKey::new(value)
                }
            }
        )*
    };
}

// Common key types: integer ids/indices, chars, and string names. A blanket
// `impl<T: Hash> From<T>` would collide with the reflexive `From<ChildKey>`, so
// the ergonomic conversions are spelled out for the types keys are drawn from.
child_key_from!(
    u8, u16, u32, u64, usize, i8, i16, i32, i64, isize, char, &str, String
);

/// Erase a view-held `Rc<dyn Fn(&mut State)>` app-state callback into the
/// widget-held [`ErasedCallback`] adapter the interactive widgets invoke during
/// the event pass.
///
/// The closure recovers the concrete `State` from the type-erased [`EventCtx`]
/// with [`EventCtx::state_mut`] (the downcast happens *inside* the adapter), so
/// the widget itself stays non-generic over `State`. Closures aren't comparable,
/// so `build`/`rebuild` reinstall the adapter unconditionally — it's cheap.
pub(crate) fn erase_callback<State: 'static>(callback: &Rc<dyn Fn(&mut State)>) -> ErasedCallback {
    let callback = callback.clone();
    Box::new(move |ctx: &mut EventCtx| {
        let state = ctx.state_mut::<State>();
        callback(state);
    })
}

/// Like [`erase_callback`], but for callbacks that also carry a value argument
/// (Checkbox's `bool`, Slider's `f64`).
pub(crate) fn erase_callback_arg<State: 'static, A: 'static>(
    callback: &TypedArgCallback<State, A>,
) -> ErasedArgCallback<A> {
    let callback = callback.clone();
    Box::new(move |ctx: &mut EventCtx, arg: A| {
        let state = ctx.state_mut::<State>();
        callback(state, arg);
    })
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s element.
///
/// The element (`Box<dyn Widget>`) is stored double-boxed so a later
/// [`rebuild_child`] can recover it as `&mut Box<dyn Widget>` — the type
/// `AnyView`'s `rebuild` needs to swap the widget on a concrete-type change.
pub(crate) fn build_child<State: 'static>(
    view: &AnyView<State>,
    ctx: &mut BuildCtx<'_>,
) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

/// Reconcile one [`AnyView`] child in place through its `ChildPod`.
///
/// A type-swapped child (see [`rebuild_child_tracked`]) that still holds a
/// recorded capture has that capture dropped: `pod.set_active(false)`, no
/// synthetic `Cancel`. The old armed widget was torn down inside
/// `AnyView::rebuild` — its state died with it — and the fresh widget in its
/// place never saw the original `Down`, so there is nothing to unwind; this
/// mirrors [`rebuild_children`]'s documented type-swap semantics for the
/// single-child wrappers (`Padding`/`Align`/`SizedBox`, and the interactive
/// widgets' own label/track children) that call this instead of
/// `rebuild_children`.
pub(crate) fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let (flags, swapped) = rebuild_child_tracked(prev, next, pod, ctx);
    if swapped && pod.is_active() {
        pod.set_active(false);
    }
    flags
}

/// Like [`rebuild_child`], but also reports whether the rebuild *replaced* the
/// underlying widget (an [`AnyView`] concrete-type swap) rather than mutating it
/// in place.
///
/// The swap flag drives both callers' capture bookkeeping: [`rebuild_children`]'s
/// (multi-child `Vec` containers — `Flex`/`Stack`) and [`rebuild_child`]'s
/// (single-child wrappers). A fresh widget swapped in at a still-captured
/// index/pod never saw the original `Down`, so its stale `active` path is
/// dropped without a synthetic `Cancel` (there is nothing armed to unwind).
/// Detection compares the boxed element's concrete
/// [`TypeId`](std::any::TypeId) across the rebuild.
fn rebuild_child_tracked<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> (ChangeFlags, bool) {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("layout-container child element is a boxed AnyView widget");
    let before = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    let flags = next.rebuild(prev, element, ctx);
    let after = {
        let any: &dyn Any = &**element;
        any.type_id()
    };
    (flags, before != after)
}

/// Tear down one [`AnyView`] child through its `ChildPod`.
///
/// A pod still holding an in-flight capture ([`ChildPod::is_active`]) is
/// [cancelled](cancel_pod) before teardown, so an armed widget dropped mid-gesture
/// (e.g. the active row truncated out of a shrinking list) unwinds its state
/// machine instead of vanishing with no terminating `Up`/`Cancel`.
pub(crate) fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if pod.is_active() {
        cancel_pod(pod);
        pod.set_active(false);
    }
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

/// Deliver a synthetic [`PointerPhase::Cancel`] to a captured child whose
/// in-flight gesture a structural rebuild has invalidated, so its
/// (g2-hardened) state machine unwinds instead of firing on a later `Up`.
///
/// # Cancel-during-rebuild contract
///
/// The rebuild pass runs over a [`BuildCtx`], not an [`EventCtx`] — there is no
/// application state in scope. This is sound *only because a `Cancel` handler
/// must never read application state* (`EventCtx::state_mut`): post-g2 every
/// interactive widget's `Cancel` arm only clears internal flags. That invariant
/// lets this build a minimal [`EventCtx`] over a throwaway `()` state to drive
/// the unwind; a `Cancel` handler that reached for real state would panic here
/// on the `()` downcast — a deliberate tripwire, not a silent corruption.
fn cancel_pod(pod: &mut ChildPod) {
    let mut dummy_state = ();
    let mut ctx = EventCtx::new(&mut dummy_state, pod.origin(), pod.size());
    // Position is irrelevant to a `Cancel` (handlers never hit-test on it).
    let cancel = InputEvent::Pointer(PointerEvent {
        phase: PointerPhase::Cancel,
        position: Point::ZERO,
        button: PointerButton::Primary,
    });
    pod.event_child(&mut ctx, &cancel);
}

/// Cancel-and-clear every surviving child's recorded interaction paths after a
/// structural change — both the capture (`active`) path and the focus
/// (`focused`) path.
///
/// Called by [`rebuild_children`]'s positional and keyed reconcilers once a
/// structural change is detected: the recorded paths can no longer be trusted
/// (a length shift, type swap, or keyed reorder moves widgets under the paths),
/// so any surviving armed widget is unwound via [`cancel_pod`] and its `active`
/// flag dropped, and any focused child has its `focused` flag dropped.
///
/// # Focus vs. capture: why one synthesizes a `Cancel` and the other does not
///
/// Capture state lives *inside* the widget (an `armed`/`pressed` flag its own
/// event arms), so dropping the recorded path requires a synthetic [`Cancel`]
/// ([`cancel_pod`]) to unwind that internal machine — otherwise it would fire on
/// a later hit-tested `Up`. Focus state, by contrast, is *reflected* from the pod
/// flag into the widget each event (`EventCtx::has_focus`, threaded through
/// [`ChildPod::event_child`]) rather than latched internally, so clearing the pod
/// flag is enough — there is no widget-internal blur to drive, and (per the g5
/// contract) a `Cancel` handler must not touch app state anyway.
///
/// **RenderRoot desync note.** Clearing `focused` here does *not* notify
/// [`RenderRoot`](forgekit_core::RenderRoot): the rebuild pass runs over a
/// [`BuildCtx`], with no `RenderRoot` in scope, exactly as the g5 capture-cancel
/// cannot reset `RenderRoot::pointer_captured`. So the root's
/// `focus_active`/`ime_state` stay momentarily stale after a structural blur and
/// self-correct on the next event pass (a `Down` re-evaluates focus/blur; a
/// focus-routed event finds no focused pod and is ignored). This is the
/// conservative, correct-by-convergence behavior — the same transient g5 already
/// accepts for capture — not a new desync class.
fn cancel_active_children(pods: &mut [ChildPod]) {
    for pod in pods.iter_mut() {
        if pod.is_active() {
            cancel_pod(pod);
            pod.set_active(false);
        }
        if pod.is_focused() {
            pod.set_focused(false);
        }
    }
}

/// Diff a child list against its live `ChildPod`s, extracting each child's
/// [`AnyView`] through `view_of` (identity for a plain `Vec<AnyView>`, `|c|
/// &c.view` for Flex's `FlexChild` wrapper) and its optional [`ChildKey`]
/// through `key_of` (`|_| None` for keyless containers, `|c| c.key` for Flex) —
/// the one shared reconciler for every multi-child container.
///
/// **Positional vs. keyed.** With *no* child carrying a key this is a positional
/// diff (see [`rebuild_children_positional`]); with keys present it matches
/// old↔new by key so reorders/inserts preserve widget identity and state (see
/// [`rebuild_children_keyed`]). Keys are all-or-nothing per list: a list that
/// mixes keyed and unkeyed children, or repeats a key, trips a `debug_assert`
/// and falls back to the positional path (correct, just identity-blind).
///
/// **Structural change cancels in-flight interaction.** Either path treats a
/// child-count change, an in-place type swap, or (keyed only) a reorder as a
/// structural edit that invalidates the recorded capture/focus paths, unwinding
/// them via [`cancel_active_children`]; a structural-change-free rebuild leaves
/// the recorded paths untouched, so an ordinary every-frame rebuild never breaks
/// a captured drag or dismisses the keyboard.
pub(crate) fn rebuild_children<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
    key_of: impl Fn(&C) -> Option<ChildKey>,
) -> ChangeFlags {
    let any_keyed = prev.iter().chain(next.iter()).any(|c| key_of(c).is_some());
    if !any_keyed {
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }
    // v1 keys are all-or-nothing per list: a mixed list has no well-defined
    // match for its unkeyed members, so fall back to positional (identity-blind
    // but correct) rather than guess. A debug build flags the misuse loudly.
    let all_keyed = prev.iter().chain(next.iter()).all(|c| key_of(c).is_some());
    if !all_keyed {
        debug_assert!(
            false,
            "keyed child list mixes keyed and unkeyed children; \
             falling back to positional reconciliation"
        );
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }
    rebuild_children_keyed(prev, next, pods, ctx, view_of, key_of)
}

/// The positional reconciler: common indices rebuild in place, a grown tail is
/// built, a shrunk tail is torn down and dropped. Length changes signal
/// `LAYOUT | PAINT`.
///
/// **Structural change cancels in-flight gestures.** Positional reconciliation
/// misroutes a captured gesture across a structural edit: a length shift moves
/// the armed widget to a different logical index, and a type swap replaces the
/// widget under a still-recorded path. On any child-count change *or* in-place
/// type swap, every surviving captured/focused child is
/// [cleared](cancel_active_children) so it unwinds rather than firing on a
/// hit-tested `Up`; a swapped-in fresh widget (which never saw `Down`) just has
/// its stale path dropped; and a truncated active pod is cancelled in
/// [`teardown_child`]. A structural-change-free rebuild leaves the recorded
/// paths untouched, so an ordinary every-frame rebuild never breaks a captured
/// drag.
fn rebuild_children_positional<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
) -> ChangeFlags {
    let mut flags = ChangeFlags::NONE;
    let common = prev.len().min(next.len());
    let mut swapped = false;
    for i in 0..common {
        let (child_flags, child_swapped) =
            rebuild_child_tracked(view_of(&prev[i]), view_of(&next[i]), &mut pods[i], ctx);
        flags |= child_flags;
        if child_swapped {
            // Fresh widget at this slot: drop the stale capture, nothing to cancel.
            pods[i].set_active(false);
            swapped = true;
        }
    }
    if next.len() > prev.len() {
        for child in &next[prev.len()..] {
            pods.push(build_child(view_of(child), ctx));
        }
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    } else if next.len() < prev.len() {
        for (offset, child) in prev[next.len()..].iter().enumerate() {
            teardown_child(view_of(child), &mut pods[next.len() + offset], ctx);
        }
        pods.truncate(next.len());
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    // A structural change — child-count shift or an in-place type swap —
    // invalidates any surviving in-flight capture (truncated active pods were
    // already cancelled in teardown; swapped slots were cleared above).
    if prev.len() != next.len() || swapped {
        cancel_active_children(pods);
    }
    flags
}

/// The keyed reconciler: match old↔new children by [`ChildKey`] so reorders and
/// inserts preserve each surviving child's live widget (and thus its internal
/// state) instead of rebuilding whatever happens to sit at the same index.
///
/// Matched children are *relocated* into the new order — their `ChildPod` (boxed
/// widget + geometry) is moved, then rebuilt in place against its own previous
/// view. Unmatched new keys are built fresh; unmatched old keys are torn down
/// (with [`teardown_child`]'s cancel-if-active). A duplicate key (old or new)
/// trips a `debug_assert` and falls back to the positional path.
///
/// **Any move is a structural change.** A reorder, insert, or removal
/// invalidates the recorded capture/focus paths (v1 is conservative: it does not
/// try to carry an in-flight gesture or the keyboard across a moved row), so on
/// any such edit every surviving child's paths are cleared via
/// [`cancel_active_children`]. A same-keys, same-order rebuild is *not*
/// structural: the recorded paths survive, exactly mirroring the positional
/// path's content-only rebuild. A future surgical-preserve option could relocate
/// the `active`/`focused` flag with its pod instead of clearing it.
fn rebuild_children_keyed<State: 'static, C>(
    prev: &[C],
    next: &[C],
    pods: &mut Vec<ChildPod>,
    ctx: &mut BuildCtx<'_>,
    view_of: impl Fn(&C) -> &AnyView<State>,
    key_of: impl Fn(&C) -> Option<ChildKey>,
) -> ChangeFlags {
    // Old key -> old index, flagging any duplicate.
    let mut old_by_key: HashMap<ChildKey, usize> = HashMap::with_capacity(prev.len());
    let mut duplicate = false;
    for (i, child) in prev.iter().enumerate() {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if old_by_key.insert(key, i).is_some() {
            duplicate = true;
        }
    }
    // Duplicate new keys are equally ambiguous (two children claim one identity).
    let mut seen_new: HashSet<ChildKey> = HashSet::with_capacity(next.len());
    for child in next {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if !seen_new.insert(key) {
            duplicate = true;
        }
    }
    if duplicate {
        debug_assert!(
            false,
            "keyed child list has duplicate keys; falling back to positional reconciliation"
        );
        return rebuild_children_positional(prev, next, pods, ctx, view_of);
    }

    // Take ownership of the old pods so matched ones can be relocated by `take`.
    let mut old_pods: Vec<Option<ChildPod>> = pods.drain(..).map(Some).collect();
    let mut new_pods: Vec<ChildPod> = Vec::with_capacity(next.len());
    let mut flags = ChangeFlags::NONE;
    let mut structural = false;
    // The old index of the previously-matched child: if a later match resolves to
    // an *earlier* old index, the relative order changed → a reorder.
    let mut last_matched_old: Option<usize> = None;

    for child in next {
        let key = key_of(child).expect("all-keyed list checked by caller");
        if let Some(&old_index) = old_by_key.get(&key) {
            if let Some(prev_old) = last_matched_old
                && old_index < prev_old
            {
                structural = true;
            }
            last_matched_old = Some(old_index);
            let mut pod = old_pods[old_index]
                .take()
                .expect("each old key matches at most one new child (no duplicates)");
            let (child_flags, child_swapped) =
                rebuild_child_tracked(view_of(&prev[old_index]), view_of(child), &mut pod, ctx);
            flags |= child_flags;
            if child_swapped {
                // A key reused for a different concrete type: the old widget was
                // torn down inside AnyView::rebuild; drop the stale paths.
                pod.set_active(false);
                pod.set_focused(false);
                structural = true;
            }
            new_pods.push(pod);
        } else {
            // A brand-new key: build a fresh child.
            new_pods.push(build_child(view_of(child), ctx));
            structural = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
    }

    // Any old pod never taken is a removed key: tear it down (cancelling first if
    // it held an in-flight capture).
    for (old_index, slot) in old_pods.iter_mut().enumerate() {
        if let Some(mut pod) = slot.take() {
            teardown_child(view_of(&prev[old_index]), &mut pod, ctx);
            structural = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
    }

    *pods = new_pods;

    if structural {
        cancel_active_children(pods);
    }
    flags
}

/// Whether `event` is the phase that auto-releases a recorded capture
/// (`Up`/`Cancel`) — shared by [`route_event`]/[`route_event_single`].
fn releases_capture(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p)
            if matches!(p.phase, PointerPhase::Up | PointerPhase::Cancel)
    )
}

/// Whether `event` is a pointer `Down` — the phase that both opens a capture and
/// triggers blur-on-outside-tap evaluation in the routing helpers.
fn is_pointer_down(event: &InputEvent) -> bool {
    matches!(
        event,
        InputEvent::Pointer(p) if matches!(p.phase, PointerPhase::Down)
    )
}

/// Route a pointer/scroll/keyboard/IME event to a container's children.
///
/// **Focus-routed events** ([`InputEvent::Key`]/[`InputEvent::Ime`]) bypass hit
/// testing entirely: they go straight to the child holding the recorded focus
/// path ([`ChildPod::is_focused`]), or are ignored if none does. This is the
/// focus mirror of the capture fast-path.
///
/// For **pointer/scroll**: a captured gesture goes straight to the recorded
/// active child (capture auto-releases on `Up`/`Cancel`). Otherwise the children
/// are hit-tested in reverse paint order — topmost (last-painted) first — and the
/// first child that both contains the point and reports [`EventResult::Handled`]
/// consumes it. This is the z-order convention Flex establishes and Stack shares.
///
/// **Blur-on-outside-tap:** a pointer `Down` that does not (re)establish focus on
/// the child it hits clears every focused child in this container. At the nearest
/// common ancestor of a stale focus branch and the tapped branch, this breaks the
/// recorded focus chain (a `Down` inside the still-focused child keeps it — that
/// child stays focused and is not cleared). Deeper stale flags below a cleared
/// link are unreachable and are corrected the next time focus enters that subtree
/// (a focus request re-records the whole chain).
pub(crate) fn route_event(
    children: &mut [ChildPod],
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_focus_routed() {
        if let Some(pod) = children.iter_mut().find(|p| p.is_focused()) {
            return pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if let Some(pod) = children.iter_mut().find(|p| p.is_active()) {
        return route_event_single(pod, ctx, event);
    }
    let position = event.position();
    let mut handled = EventResult::Ignored;
    // The index of the hit child *if* it holds focus after dispatch — the one
    // focused child blur-on-Down must preserve.
    let mut kept_focus: Option<usize> = None;
    let n = children.len();
    for i in (0..n).rev() {
        if children[i].contains(position)
            && children[i].event_child(ctx, event) == EventResult::Handled
        {
            if children[i].is_focused() {
                kept_focus = Some(i);
            }
            handled = EventResult::Handled;
            break;
        }
    }
    if is_pointer_down(event) {
        for (i, pod) in children.iter_mut().enumerate() {
            if Some(i) != kept_focus && pod.is_focused() {
                pod.set_focused(false);
            }
        }
    }
    handled
}

/// Route a pointer/scroll event to a container's single child.
///
/// Mirrors [`route_event`] for the one-child wrappers (`Padding`/`Align`/
/// `SizedBox`): a captured gesture is forwarded to `pod` unconditionally —
/// regardless of whether the event's position still falls within the child's
/// bounds — with the active path cleared on `Up`/`Cancel`; otherwise the child
/// only receives the event if it contains the point. Re-hit-testing
/// `pod.contains()` on every event instead of consulting [`ChildPod::is_active`]
/// is the bug this helper exists to prevent — see `ChildPod::contains`'s docs.
pub(crate) fn route_event_single(
    pod: &mut ChildPod,
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_focus_routed() {
        // Focus-routed events go to the child only if it holds the focus path.
        if pod.is_focused() {
            return pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if pod.is_active() {
        let result = pod.event_child(ctx, event);
        if releases_capture(event) {
            pod.set_active(false);
        }
        return result;
    }
    let inside = pod.contains(event.position());
    let result = if inside {
        pod.event_child(ctx, event)
    } else {
        EventResult::Ignored
    };
    // Blur-on-outside-tap: a `Down` that lands *outside* the (single) focused
    // child drops its recorded focus path. A `Down` inside the child keeps focus
    // (the child re-requests it, or simply stays the focused widget).
    if is_pointer_down(event) && !inside && pod.is_focused() {
        pod.set_focused(false);
    }
    result
}

/// Shared, GPU-free fixtures for the container layout/paint/event tests: a
/// fixed-size [`Leaf`], a distinctive swap partner, an event-recording
/// [`Probe`], and a recording [`RecordingScene`].
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use forgekit_core::{BoxConstraints, LayoutCtx, PaintCtx, PaintScene, any};
    use kurbo::{Point, Size};
    use peniko::Color;

    /// A leaf view of fixed intrinsic size that fills a rect on paint.
    pub(crate) struct Leaf {
        intrinsic: Size,
    }

    /// Build a [`Leaf`] with the given intrinsic width/height.
    pub(crate) fn leaf(width: f64, height: f64) -> Leaf {
        Leaf {
            intrinsic: Size::new(width, height),
        }
    }

    /// A [`Leaf`], type-erased for a `()`-state container.
    pub(crate) fn leaf_any(width: f64, height: f64) -> AnyView<()> {
        any(leaf(width, height))
    }

    impl Leaf {
        /// Erase this leaf into an `AnyView<()>`.
        pub(crate) fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`Leaf`].
    pub(crate) struct LeafWidget {
        intrinsic: Size,
    }

    impl View<()> for Leaf {
        type Element = LeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> LeafWidget {
            LeafWidget {
                intrinsic: self.intrinsic,
            }
        }
        fn rebuild(
            &self,
            prev: &Self,
            element: &mut LeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            if prev.intrinsic != self.intrinsic {
                element.intrinsic = self.intrinsic;
                ChangeFlags::LAYOUT
            } else {
                ChangeFlags::NONE
            }
        }
    }

    impl Widget for LeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.intrinsic)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }

    /// A distinctive view whose widget always reports a 7x7 size — used to prove
    /// an `AnyView` type-swap actually replaced the widget.
    pub(crate) struct SwapLeaf;

    /// Build the [`SwapLeaf`] swap partner.
    pub(crate) fn swap_leaf() -> SwapLeaf {
        SwapLeaf
    }

    impl SwapLeaf {
        /// Erase this view into an `AnyView<()>`.
        pub(crate) fn into_any(self) -> AnyView<()> {
            any(self)
        }
    }

    /// Retained widget for [`SwapLeaf`].
    pub(crate) struct SwapLeafWidget;

    impl View<()> for SwapLeaf {
        type Element = SwapLeafWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwapLeafWidget {
            SwapLeafWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut SwapLeafWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for SwapLeafWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(7.0, 7.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// A view whose widget fills its constraints and, on any event, records its
    /// `id` into the `Vec<u32>` application state and reports `Handled`. Used to
    /// prove event routing order.
    pub(crate) struct Probe {
        id: u32,
    }

    /// Build a [`Probe`] tagged with `id`.
    pub(crate) fn probe(id: u32) -> Probe {
        Probe { id }
    }

    impl Probe {
        /// Erase this probe into an `AnyView<Vec<u32>>`.
        pub(crate) fn into_any(self) -> AnyView<Vec<u32>> {
            any(self)
        }
    }

    /// Retained widget for [`Probe`].
    pub(crate) struct ProbeWidget {
        id: u32,
    }

    impl View<Vec<u32>> for Probe {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget { id: self.id }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            ctx.state_mut::<Vec<u32>>().push(self.id);
            ctx.request_redraw();
            EventResult::Handled
        }
    }

    /// A GPU-free [`PaintScene`] that records filled rects in paint order.
    #[derive(Default)]
    pub(crate) struct RecordingScene {
        pub(crate) rects: Vec<(Point, Size)>,
    }

    impl PaintScene for RecordingScene {
        fn fill_rect(&mut self, origin: Point, size: Size, _color: Color) {
            self.rects.push((origin, size));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }
}

/// Mechanism-level tests for [`rebuild_child`]'s type-swap capture handling
/// (re-review round 1): every single-child container (`Padding`/`Align`/
/// `SizedBox`, interactive widgets' labels) reconciles its child through this
/// helper, so the clear-on-swap behavior is proven once here at the shared
/// substrate, and each container's own test module (see `padding`/`align`/
/// `sized`) proves it end to end through real event routing/geometry.
#[cfg(test)]
mod tests {
    use super::test_support::{leaf_any, swap_leaf};
    use super::*;
    use forgekit_core::BuildCtx;

    fn ctx(counter: &mut u64) -> BuildCtx<'_> {
        BuildCtx::new(counter)
    }

    #[test]
    fn rebuild_child_clears_active_on_type_swap() {
        // A type swap (Leaf -> SwapLeaf, different concrete types) tears down
        // the old widget inside AnyView::rebuild and builds a fresh one in its
        // place; a still-active pod must have its stale capture dropped so a
        // later event isn't misrouted into the fresh widget.
        let mut counter = 0u64;
        let prev: AnyView<()> = leaf_any(10.0, 10.0);
        let mut pod = build_child(&prev, &mut ctx(&mut counter));
        pod.set_active(true);

        let next: AnyView<()> = swap_leaf().into_any();
        rebuild_child::<()>(&prev, &next, &mut pod, &mut ctx(&mut counter));

        assert!(
            !pod.is_active(),
            "a type-swapped child's stale capture must be dropped"
        );
    }

    #[test]
    fn rebuild_child_preserves_active_on_same_type_rebuild() {
        // Negative guard against over-clearing: a same-type, content-only
        // rebuild (no AnyView type swap) must NOT touch an in-flight capture —
        // mirrors rebuild_children's content_only_rebuild_preserves_captured_drag.
        let mut counter = 0u64;
        let prev: AnyView<()> = leaf_any(10.0, 10.0);
        let mut pod = build_child(&prev, &mut ctx(&mut counter));
        pod.set_active(true);

        let next: AnyView<()> = leaf_any(20.0, 20.0);
        rebuild_child::<()>(&prev, &next, &mut pod, &mut ctx(&mut counter));

        assert!(
            pod.is_active(),
            "a content-only rebuild must not clear an in-flight capture"
        );
    }
}

/// Focus-path routing tests for [`route_event`]/[`route_event_single`]: Key/IME
/// events reach only the focused child (including through a nested container),
/// blur-on-outside-tap breaks the chain, a second focus request moves focus,
/// `ApplyEditingState` routes to the focused child, a published IME surface
/// bubbles up, and the capture and focus paths stay independent.
#[cfg(test)]
mod focus_tests {
    use super::*;
    use forgekit_core::{
        BoxConstraints, EditingState, EventCtx, ImeEvent, ImeState, Key, KeyEvent, LayoutCtx,
        Modifiers, PaintCtx, PaintScene,
    };
    use kurbo::{Point, Rect, Size};

    /// A leaf that: on a pointer `Down` optionally requests focus and records
    /// `id + 100`; on a `Move` (capture path) records `id + 200`; on a Key/IME
    /// event records `id` and optionally publishes an IME surface. All state is a
    /// shared `Vec<u32>` so tests can assert *which* leaf saw *what*.
    struct KeyLeaf {
        id: u32,
        takes_focus: bool,
        publish_ime: bool,
    }

    impl Widget for KeyLeaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            match event {
                InputEvent::Pointer(p) => match p.phase {
                    PointerPhase::Down => {
                        if self.takes_focus {
                            ctx.request_focus();
                        }
                        ctx.state_mut::<Vec<u32>>().push(self.id + 100);
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        ctx.state_mut::<Vec<u32>>().push(self.id + 200);
                        EventResult::Handled
                    }
                    _ => EventResult::Handled,
                },
                InputEvent::Key(_) | InputEvent::Ime(_) => {
                    ctx.state_mut::<Vec<u32>>().push(self.id);
                    if self.publish_ime {
                        ctx.publish_ime_state(ImeState {
                            active: true,
                            editing: EditingState {
                                text: format!("leaf{}", self.id),
                                selection_base: 1,
                                selection_extent: 1,
                                composing_base: -1,
                                composing_extent: -1,
                            },
                            caret: Some(Rect::new(0.0, 0.0, 1.0, 10.0)),
                        });
                    }
                    EventResult::Handled
                }
                _ => EventResult::Ignored,
            }
        }
    }

    /// A minimal multi-child container: stacks its children vertically (each 10
    /// tall at the given width) and routes events through [`route_event`] — the
    /// same helper the real `Flex`/`Stack` use — so focus routing is exercised
    /// through a nested container.
    struct Nest {
        children: Vec<ChildPod>,
    }

    impl Widget for Nest {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            let w = bc.max().width;
            for (i, pod) in self.children.iter_mut().enumerate() {
                pod.set_origin(Point::new(0.0, i as f64 * 10.0));
                pod.layout_child(ctx, &BoxConstraints::tight(Size::new(w, 10.0)));
            }
            Size::new(w, self.children.len() as f64 * 10.0)
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            route_event(&mut self.children, ctx, event)
        }
    }

    fn leaf_pod(id: u32, takes_focus: bool, publish_ime: bool) -> ChildPod {
        ChildPod::new(Box::new(KeyLeaf {
            id,
            takes_focus,
            publish_ime,
        }))
    }

    /// Lay out a slice of pods vertically (10 tall each at width 100) so hit
    /// testing distinguishes them: child `i` occupies `y ∈ [i*10, i*10+10)`.
    fn lay_out_vertically(pods: &mut [ChildPod]) {
        let mut lctx = LayoutCtx::new();
        for (i, pod) in pods.iter_mut().enumerate() {
            pod.set_origin(Point::new(0.0, i as f64 * 10.0));
            pod.layout_child(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 10.0)));
        }
    }

    fn down(y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(5.0, y),
            button: PointerButton::Primary,
        })
    }

    fn mv(y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Move,
            position: Point::new(5.0, y),
            button: PointerButton::Primary,
        })
    }

    fn key() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character("a".to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn ime_apply() -> InputEvent {
        InputEvent::Ime(ImeEvent::ApplyEditingState(EditingState {
            text: "sync".to_string(),
            selection_base: 4,
            selection_extent: 4,
            composing_base: -1,
            composing_extent: -1,
        }))
    }

    // `state` must be a concrete `&mut Vec<u32>`: it is erased to `&mut dyn Any`
    // and the leaves recover it with `state_mut::<Vec<u32>>()`, so a slice would
    // fail the downcast.
    #[allow(clippy::ptr_arg)]
    fn route(children: &mut [ChildPod], state: &mut Vec<u32>, event: &InputEvent) -> EventResult {
        let mut ctx = EventCtx::new(state, Point::ZERO, Size::new(100.0, 100.0));
        route_event(children, &mut ctx, event)
    }

    #[test]
    fn key_routes_only_to_focused_child() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        // Tap child 2 (y in [10,20)) → it requests focus.
        route(&mut children, &mut state, &down(15.0));
        assert!(children[1].is_focused());
        assert!(!children[0].is_focused());
        state.clear();

        // A Key event reaches ONLY the focused child (id 2), never child 1.
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn key_reaches_focused_leaf_through_nested_container() {
        // Outer container holds one Nest; the Nest holds two leaves.
        let inner = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        let mut nest = Nest { children: inner };
        // Lay out the nest so its inner children get real geometry.
        {
            let mut lctx = LayoutCtx::new();
            nest.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 20.0)));
        }
        let mut outer = vec![ChildPod::new(Box::new(nest))];
        outer[0].set_origin(Point::ZERO);
        {
            let mut lctx = LayoutCtx::new();
            outer[0].layout_child(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 20.0)));
        }
        let mut state = Vec::new();

        // Tap the second inner leaf (y in [10,20)) → focus chain nest→leaf2.
        route(&mut outer, &mut state, &down(15.0));
        assert!(outer[0].is_focused(), "the nest records the focus path");
        state.clear();

        // Key routes outer→nest→leaf2 only.
        route(&mut outer, &mut state, &key());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn blur_on_outside_tap_clears_focus_chain() {
        // Child 1 takes focus; child 2 does NOT (a non-editable widget).
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        assert!(children[0].is_focused());

        // Tap child 2 (does not take focus) → blur clears the focus chain.
        route(&mut children, &mut state, &down(15.0));
        assert!(
            !children[0].is_focused(),
            "outside tap blurs the focused child"
        );
        assert!(!children[1].is_focused());
        state.clear();

        // A Key event now reaches nobody.
        let result = route(&mut children, &mut state, &key());
        assert_eq!(result, EventResult::Ignored);
        assert!(state.is_empty());
    }

    #[test]
    fn tap_inside_focused_widget_keeps_focus() {
        // A focused widget re-tapped keeps focus (Down inside focused widget).
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        route(&mut children, &mut state, &down(5.0)); // tap child 1 again
        assert!(
            children[0].is_focused(),
            "a tap inside the focused widget keeps focus"
        );
        state.clear();
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![1]);
    }

    #[test]
    fn second_focus_request_moves_focus() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(5.0)); // focus child 1
        assert!(children[0].is_focused());
        route(&mut children, &mut state, &down(15.0)); // focus child 2
        assert!(children[1].is_focused());
        assert!(!children[0].is_focused(), "focus moved off child 1");
        state.clear();

        route(&mut children, &mut state, &key());
        assert_eq!(
            state,
            vec![2],
            "key now reaches only the newly focused child"
        );
    }

    #[test]
    fn ime_apply_routes_to_focused_child() {
        let mut children = vec![leaf_pod(1, true, false), leaf_pod(2, true, false)];
        lay_out_vertically(&mut children);
        let mut state = Vec::new();

        route(&mut children, &mut state, &down(15.0)); // focus child 2
        state.clear();
        route(&mut children, &mut state, &ime_apply());
        assert_eq!(state, vec![2]);
    }

    #[test]
    fn capture_and_focus_paths_are_independent() {
        // Child 1 holds the capture (active) path; child 2 holds the focus path.
        let mut children = vec![leaf_pod(1, false, false), leaf_pod(2, false, false)];
        lay_out_vertically(&mut children);
        children[0].set_active(true);
        children[1].set_focused(true);
        let mut state = Vec::new();

        // A Move routes down the capture path → child 1 only (id+200).
        route(&mut children, &mut state, &mv(999.0));
        assert_eq!(state, vec![201]);
        state.clear();

        // A Key routes down the focus path → child 2 only (id).
        route(&mut children, &mut state, &key());
        assert_eq!(state, vec![2]);

        // Both paths survive intact.
        assert!(children[0].is_active());
        assert!(children[1].is_focused());
    }
}
