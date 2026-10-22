//! `native_scaffold(body)`: the app-facing chrome builder (decision C1).
//!
//! The tab bar, title, leading item, actions and toolbar are the *host's*, not a
//! `platform_view` slot: the scaffold builds a `ChromeSpec` from its builder
//! methods and publishes it to the process-wide slot in `crate::chrome`, where
//! an arm (or, on a target without one, the fallback chrome) owns the real
//! bars. The body renders full-bleed. A second live scaffold publishes
//! nothing and shows the refusal banner above its body.
//!
//! # Controlled, like the bare tab bar
//!
//! The bars never navigate. A tap reports a [`ChromeEvent`]; the app routes
//! and the confirmed `selected` comes back through the next build — exactly
//! [`NativeTabBarView`](super::NativeTabBarView)'s contract and handler shape
//! (`Fn + Send + Sync`, no `&mut State`).
//!
//! # The toolbar
//!
//! `.toolbar(ToolbarSpec)` adds a bottom action strip: iOS navigation-controller
//! toolbar items, Android floating action buttons, a frust-drawn row above the
//! fallback tab row. Its actions report through `.on_action`/`.on_menu` like
//! the top bar's, so every action id must be unique across both: a toolbar
//! entry repeating an earlier id is dropped from the published spec, logged
//! once per change and named by a one-line banner above the body.
//!
//! # Insets
//!
//! On a target with a chrome arm the host already reports the bars through the
//! window insets (iOS safe area, the toolbar included) or shrinks the surface
//! (Android; its floating action buttons float over the body and take no
//! inset), so the body uses the facade's `safe_area` as usual. The scaffold
//! adds no padding of its own: do not double-pad. The fallback bars
//! (`crate::chrome::fallback`) take the top and bottom insets themselves and
//! lay the body out between them with those edges consumed, so the same
//! `safe_area` body pads only the sides.
//!
//! # Outcome changes off the build path
//!
//! The fallback bars and the `native` flag are reconciled only by `sync`, on
//! build and rebuild. An outcome that changes elsewhere (an arm's failure
//! taken by a scroll flush, a NoHost retry a flush turns into an install, an
//! arm's own late failure) bumps the chrome epoch, which owes the shell the
//! frame whose rebuild re-runs `sync`. A flush bumps only when its outcome
//! crosses between native and fallback, never per scroll frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use frust::authoring::{build_child, rebuild_child, route_event_single, teardown_child};
use frust::{RwSignal, ScrollController, ScrollInfo, ScrollSubscription, Set, Theme, use_context};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget, WindowInsets,
};
use kurbo::{Point, Size};
use peniko::{Brush, Color, Gradient};

use super::builders::{TabId, TabItem, banner_placeholder};
use super::signals::{ChromeHandlers, on_chrome};
use super::theme;
use crate::chrome::fallback::{
    fallback_tab_row, fallback_toolbar_row, fallback_top_bar, teardown_tab_row,
    teardown_toolbar_row, teardown_top_bar,
};
use crate::chrome::{
    self, ActionId, ActionItem, BARS_FLOAT_OVER_BODY, ChromeEvent, ChromeGeneration, ChromeSpec,
    Leading, MenuItemId, PublishOutcome, ScrollState, ToolbarSpec, TopBarSpec, shown_toolbar,
};

/// How far past a bar's inset line the edge fade reaches into the body, in
/// logical pixels.
const EDGE_FADE_TAIL: f64 = 24.0;
/// The fade's alpha at the screen edge and where it crosses the inset line.
const EDGE_FADE_EDGE_ALPHA: f32 = 0.92;
const EDGE_FADE_INSET_ALPHA: f32 = 0.6;
/// The [`EdgeFade::strong`] alphas.
const STRONG_FADE_EDGE_ALPHA: f32 = 1.0;
const STRONG_FADE_INSET_ALPHA: f32 = 0.88;

/// The edge fade [`NativeScaffoldView::edge_fade`] paints under the floating
/// iOS bars: an optional colour, the alpha at the screen edge, the alpha where
/// the band crosses the bar's inset line and how far it reaches past it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeFade {
    color: Option<Color>,
    edge_alpha: f32,
    bar_alpha: f32,
    tail: f64,
}

impl Default for EdgeFade {
    /// The theme surface colour, 0.92 at the edge, 0.6 at the bar line and a
    /// 24 px tail.
    fn default() -> Self {
        Self {
            color: None,
            edge_alpha: EDGE_FADE_EDGE_ALPHA,
            bar_alpha: EDGE_FADE_INSET_ALPHA,
            tail: EDGE_FADE_TAIL,
        }
    }
}

impl EdgeFade {
    /// The stronger look tried on device: opaque at the edge, 0.88 at the bar
    /// line.
    pub fn strong() -> Self {
        Self {
            edge_alpha: STRONG_FADE_EDGE_ALPHA,
            bar_alpha: STRONG_FADE_INSET_ALPHA,
            ..Self::default()
        }
    }

    /// Fade in `color` instead of the theme surface.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// The alpha at the screen edge, clamped to `0.0..=1.0` (NaN: the default).
    pub fn edge_alpha(mut self, alpha: f32) -> Self {
        self.edge_alpha = clamp_alpha(alpha, EDGE_FADE_EDGE_ALPHA);
        self
    }

    /// The alpha where the band crosses the bar's inset line, clamped to
    /// `0.0..=1.0` (NaN: the default).
    pub fn bar_alpha(mut self, alpha: f32) -> Self {
        self.bar_alpha = clamp_alpha(alpha, EDGE_FADE_INSET_ALPHA);
        self
    }

    /// Logical pixels the fade reaches past the inset; non-finite or negative
    /// becomes 0.
    pub fn tail(mut self, tail: f64) -> Self {
        self.tail = if tail.is_finite() && tail > 0.0 {
            tail
        } else {
            0.0
        };
        self
    }

    /// This config with its colour resolved.
    fn resolve(self) -> ResolvedFade {
        ResolvedFade {
            color: self.color.unwrap_or_else(default_fade_color),
            edge_alpha: self.edge_alpha,
            bar_alpha: self.bar_alpha,
            tail: self.tail,
        }
    }
}

/// `alpha` clamped to `0.0..=1.0`, `fallback` for NaN.
fn clamp_alpha(alpha: f32, fallback: f32) -> f32 {
    if alpha.is_nan() {
        fallback
    } else {
        alpha.clamp(0.0, 1.0)
    }
}

/// An [`EdgeFade`] with its colour fixed at build time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedFade {
    color: Color,
    edge_alpha: f32,
    bar_alpha: f32,
    tail: f64,
}

/// The refusal banner's label.
const REFUSAL_LABEL: &str = "one native_scaffold per app";
/// Its description.
const REFUSAL_DESCRIPTION: &str = "this scaffold publishes no chrome";
/// The repeated-id banner's label.
const REPEAT_LABEL: &str = "chrome action ids must be unique";
/// Its description.
const REPEAT_DESCRIPTION: &str = "toolbar entries repeating an action id are not shown";

/// App chrome around `body`: tabs, title, subtitle, leading item, actions (an
/// action with a menu is a pull-down) and a bottom toolbar. See the
/// [module docs](self) for the controlled contract and the inset rule.
///
/// ```
/// use frust::{Component, Get, RwSignal, Set, View, text};
/// use frust_native_widgets::{
///     ActionItem, Leading, TabIcon, TabId, TabItem, ToolbarSpec, native_scaffold,
/// };
///
/// #[derive(Default)]
/// struct Root;
///
/// struct RootState {
///     tab: RwSignal<String>,
/// }
///
/// impl Root {
///     fn body(tab: String) -> impl View<RootState> {
///         text(format!("Showing {tab}"))
///     }
///
///     fn tabs() -> Vec<TabItem> {
///         vec![
///             TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())),
///             TabItem::new("inbox", "Inbox", TabIcon::AppleSymbol("tray".into())),
///         ]
///     }
/// }
///
/// impl Component for Root {
///     type State = RootState;
///
///     fn init(&self) -> RootState {
///         RootState { tab: RwSignal::new("home".to_string()) }
///     }
///
///     fn build(&self, state: &mut RootState) -> impl View<RootState> {
///         let tab = state.tab;
///         native_scaffold(Self::body(tab.get()))
///             .tabs(Self::tabs(), tab.get())
///             .title("Home")
///             .leading(Leading::Back)
///             .actions(vec![ActionItem::new("add", "Add")])
///             .toolbar(ToolbarSpec::new().primary(ActionItem::new("comment", "Comment")))
///             .on_tab_select(move |id: TabId| tab.set(id.to_string()))
///     }
/// }
///
/// frust::app!(Root);
/// ```
pub fn native_scaffold<V>(body: V) -> NativeScaffoldView<V> {
    NativeScaffoldView {
        body,
        tabs: Vec::new(),
        selected: None,
        top: None,
        toolbar: None,
        scroll: None,
        fade: Some(EdgeFade::default().resolve()),
        resize_to_avoid_keyboard: true,
        handlers: ChromeHandlers::default(),
    }
}

/// The builder [`native_scaffold`] returns; a `View<State>` wherever its body
/// is.
pub struct NativeScaffoldView<V> {
    body: V,
    tabs: Vec<TabItem>,
    selected: Option<TabId>,
    top: Option<TopBarSpec>,
    toolbar: Option<ToolbarSpec>,
    scroll: Option<ScrollController>,
    fade: Option<ResolvedFade>,
    resize_to_avoid_keyboard: bool,
    handlers: ChromeHandlers,
}

impl<V> NativeScaffoldView<V> {
    /// The bottom tabs and the app-confirmed `selected` one. A `selected` id
    /// no tab carries shows no selection.
    pub fn tabs(mut self, items: Vec<TabItem>, selected: impl Into<TabId>) -> Self {
        self.tabs = items;
        self.selected = Some(selected.into());
        self
    }

    fn top_mut(&mut self) -> &mut TopBarSpec {
        self.top.get_or_insert_with(TopBarSpec::default)
    }

    /// The top bar's title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.top_mut().title = title.into();
        self
    }

    /// The top bar's subtitle.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.top_mut().subtitle = Some(subtitle.into());
        self
    }

    /// The top bar's leading item.
    pub fn leading(mut self, leading: Leading) -> Self {
        self.top_mut().leading = leading;
        self
    }

    /// The trailing actions; one built with `.menu(..)` is a pull-down.
    pub fn actions(mut self, actions: Vec<ActionItem>) -> Self {
        self.top_mut().actions = actions;
        self
    }

    /// Large-title display where the platform has one (iOS); ignored elsewhere.
    pub fn large_title(mut self, large: bool) -> Self {
        self.top_mut().large_title = large;
        self
    }

    /// The bottom toolbar (see the [module docs](self)); its ids must not
    /// repeat an action's.
    pub fn toolbar(mut self, toolbar: ToolbarSpec) -> Self {
        self.toolbar = Some(toolbar);
        self
    }

    /// Mirror `controller`'s scroll position to the bars (scroll-edge
    /// appearance, lifted app bar). Changed values only, at most one publish
    /// per frame.
    pub fn scroll(mut self, controller: ScrollController) -> Self {
        self.scroll = Some(controller);
        self
    }

    /// Fires with the *requested* tab's id when a tab other than the showing
    /// one is tapped; the app confirms by feeding `selected` back.
    pub fn on_tab_select(mut self, handler: impl Fn(TabId) + Send + Sync + 'static) -> Self {
        self.handlers.on_tab_select = Some(Arc::new(handler));
        self
    }

    /// Fires with the showing tab's id when it is tapped again.
    pub fn on_tab_reselect(mut self, handler: impl Fn(TabId) + Send + Sync + 'static) -> Self {
        self.handlers.on_tab_reselect = Some(Arc::new(handler));
        self
    }

    /// Fires when a plain action is tapped.
    pub fn on_action(mut self, handler: impl Fn(ActionId) + Send + Sync + 'static) -> Self {
        self.handlers.on_action = Some(Arc::new(handler));
        self
    }

    /// Fires with the action and the item chosen from its pull-down menu.
    pub fn on_menu(
        mut self,
        handler: impl Fn(ActionId, MenuItemId) + Send + Sync + 'static,
    ) -> Self {
        self.handlers.on_menu = Some(Arc::new(handler));
        self
    }

    /// Fires when the leading back item is tapped.
    pub fn on_back(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.handlers.on_back = Some(Arc::new(handler));
        self
    }

    /// Writes the requested tab's id into `signal` on every tab selection.
    pub fn on_tab_select_into(self, signal: RwSignal<TabId>) -> Self {
        self.on_tab_select(move |id| signal.set(id))
    }

    /// Writes the last chrome event of any kind into `signal`.
    pub fn events_into(mut self, signal: RwSignal<Option<ChromeEvent>>) -> Self {
        self.handlers.on_event = Some(Arc::new(move |event| signal.set(Some(event))));
        self
    }

    /// Paint the edge fade under the floating bars as `fade` configures it.
    /// Only drawn where the bars float over the body (iOS).
    pub fn edge_fade(mut self, fade: EdgeFade) -> Self {
        self.fade = Some(fade.resolve());
        self
    }

    /// Draw no edge fade under the floating bars.
    pub fn no_edge_fade(mut self) -> Self {
        self.fade = None;
        self
    }

    /// Whether the body shrinks by the on-screen keyboard (Flutter's
    /// `resizeToAvoidBottomInset`), default on, so a bottom-pinned composer
    /// sits on the keyboard. The body then sees no IME inset, and the
    /// frust-drawn fallback tab row is hidden while the keyboard is up (the
    /// Android arm hides its bottom navigation the same way). Off leaves the
    /// body full height under the keyboard.
    pub fn resize_to_avoid_keyboard(mut self, enabled: bool) -> Self {
        self.resize_to_avoid_keyboard = enabled;
        self
    }

    /// The spec this builder publishes, its brightness from the ambient theme.
    #[cfg(test)]
    fn spec(&self) -> ChromeSpec {
        self.spec_and_repeats().0
    }

    /// [`Self::spec`], and the ids of the toolbar entries it dropped for
    /// repeating an earlier action's.
    fn spec_and_repeats(&self) -> (ChromeSpec, Vec<ActionId>) {
        let (toolbar, repeats) = match &self.toolbar {
            Some(toolbar) => {
                let (kept, repeats) = without_repeats(self.top.as_ref(), toolbar);
                (Some(kept), repeats)
            }
            None => (None, Vec::new()),
        };
        let spec = ChromeSpec {
            tabs: self.tabs.clone(),
            selected: self.selected.clone(),
            top: self.top.clone(),
            toolbar,
            scroll: self.scroll.as_ref().and_then(controller_state),
            dark: ambient_dark(),
        };
        (spec, repeats)
    }
}

/// `toolbar` without the entries whose id a top-bar action or an earlier
/// toolbar entry already carries, and those entries' ids in order.
fn without_repeats(
    top: Option<&TopBarSpec>,
    toolbar: &ToolbarSpec,
) -> (ToolbarSpec, Vec<ActionId>) {
    let mut seen: Vec<&ActionId> = top
        .map(|top| top.actions.iter().map(ActionItem::id).collect())
        .unwrap_or_default();
    let mut repeats = Vec::new();
    let mut kept = ToolbarSpec::default();
    let primary_at = toolbar.items.len();
    for (index, entry) in toolbar.entries().enumerate() {
        if seen.contains(&entry.id()) {
            repeats.push(entry.id().clone());
            continue;
        }
        seen.push(entry.id());
        if index == primary_at {
            kept.primary = Some(entry.clone());
        } else {
            kept.items.push(entry.clone());
        }
    }
    (kept, repeats)
}

/// Which one-line banner the scaffold shows above its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Banner {
    /// Another scaffold holds the chrome: this one publishes nothing.
    Refused,
    /// Toolbar entries were dropped for repeating an action id.
    Repeated,
}

impl Banner {
    fn view<Outer: 'static>(self) -> AnyView<Outer> {
        let (label, description) = match self {
            Self::Refused => (REFUSAL_LABEL, REFUSAL_DESCRIPTION),
            Self::Repeated => (REPEAT_LABEL, REPEAT_DESCRIPTION),
        };
        banner_placeholder(None, label.to_string(), description.to_string())
    }
}

/// The app theme's brightness through the controls' fold ([`theme::is_dark`]);
/// light when no [`Theme`] is threaded, as the controls degrade.
fn ambient_dark() -> bool {
    use_context::<Theme>().as_ref().is_some_and(theme::is_dark)
}

/// The default fade colour: the threaded theme's surface, else opaque black
/// when the ambient brightness is dark and white otherwise.
fn default_fade_color() -> Color {
    match use_context::<Theme>() {
        Some(theme) => theme.scheme().surface,
        None if ambient_dark() => Color::BLACK,
        None => Color::WHITE,
    }
}

/// One fade band: its origin and size within the scaffold, and the fraction
/// of the band (from the screen edge inward) that ends at the inset line.
type EdgeBand = (Point, Size, f64);

/// The top and bottom fade bands for bar insets `top` and `bottom`; an edge
/// without an inset gets none.
fn edge_fade_bands(size: Size, top: f64, bottom: f64, tail: f64) -> [Option<EdgeBand>; 2] {
    let band = |inset: f64, at_bottom: bool| {
        (inset > 0.0).then(|| {
            let height = inset + tail;
            let y = if at_bottom { size.height - height } else { 0.0 };
            (
                Point::new(0.0, y),
                Size::new(size.width, height),
                inset / height,
            )
        })
    };
    [band(top, false), band(bottom, true)]
}

/// The fade colour to paint, if any: the bars must float and be native.
fn edge_fade_color(floats: bool, native: bool, fade: Option<ResolvedFade>) -> Option<ResolvedFade> {
    fade.filter(|_| floats && native)
}

/// A band's gradient, running from its screen edge (`top`: the upper one)
/// inward through the inset line to transparent.
fn edge_fade_brush(
    origin: Point,
    size: Size,
    solid_until: f64,
    top: bool,
    color: Color,
    (edge_alpha, bar_alpha): (f32, f32),
) -> Brush {
    let (from, to) = if top {
        (origin.y, origin.y + size.height)
    } else {
        (origin.y + size.height, origin.y)
    };
    let stops = [
        (0.0f32, color.multiply_alpha(edge_alpha)),
        (solid_until as f32, color.multiply_alpha(bar_alpha)),
        (1.0f32, color.multiply_alpha(0.0)),
    ];
    Brush::Gradient(
        Gradient::new_linear(Point::new(origin.x, from), Point::new(origin.x, to))
            .with_stops(stops),
    )
}

/// The controller's current position, if a surface is attached.
fn controller_state(controller: &ScrollController) -> Option<ScrollState> {
    controller.is_attached().then(|| ScrollState {
        offset: controller.offset(),
        max_offset: controller.max_offset(),
        viewport: controller.viewport_extent(),
    })
}

/// How a publisher's flush is queued: [`spawn_flush`] in production.
type ScheduleFlush = fn(Rc<RefCell<ScrollPublisher>>);

/// Queue `publisher`'s flush as a frame task on the UI thread.
fn spawn_flush(publisher: Rc<RefCell<ScrollPublisher>>) {
    frust::spawn_local(async move {
        // The borrow ends before `flush`'s publish result is dropped.
        let _ = publisher.borrow_mut().flush();
    });
}

/// Coalesces scroll reports into at most one publish per frame: the listener
/// records the newest value (`observe`) and the frame task publishes it
/// (`flush`).
struct ScrollPublisher {
    generation: Option<ChromeGeneration>,
    spec: ChromeSpec,
    /// Whether the outcome the scaffold last drew left the bars to the arm.
    native: bool,
    queued: bool,
    schedule: ScheduleFlush,
}

impl ScrollPublisher {
    fn new(generation: Option<ChromeGeneration>, spec: ChromeSpec, native: bool) -> Self {
        Self {
            generation,
            spec,
            native,
            queued: false,
            schedule: spawn_flush,
        }
    }

    /// Adopt what the view just published and whether it drew the bars native.
    fn adopt(&mut self, generation: Option<ChromeGeneration>, spec: ChromeSpec, native: bool) {
        self.generation = generation;
        self.spec = spec;
        self.native = native;
    }

    /// Record `info` over `viewport`, the controller's extent as of this
    /// report; `true` when a flush must be scheduled (the value changed and
    /// none is already queued).
    fn observe(&mut self, info: ScrollInfo, viewport: f64) -> bool {
        let state = ScrollState {
            offset: info.offset,
            max_offset: info.max_offset,
            viewport,
        };
        if self.spec.scroll == Some(state) {
            return false;
        }
        self.spec.scroll = Some(state);
        !std::mem::replace(&mut self.queued, true)
    }

    /// Publish the newest recorded state; a publisher that no longer holds
    /// the slot forgets its generation, so it never publishes again. An
    /// outcome that crosses between native and fallback bumps the chrome
    /// epoch once, so the scaffold re-syncs its bars.
    fn flush(&mut self) -> Option<PublishOutcome> {
        self.queued = false;
        let generation = self.generation?;
        let outcome = chrome::publish(Some(generation), self.spec.clone());
        if matches!(outcome, PublishOutcome::Stale | PublishOutcome::Refused(_)) {
            self.generation = None;
            return Some(outcome);
        }
        let native = !draws_fallback(&outcome);
        if native != self.native {
            self.native = native;
            chrome::bump_epoch();
        }
        Some(outcome)
    }
}

/// The live scroll subscription and the publisher its listener feeds.
struct ScrollLink {
    publisher: Rc<RefCell<ScrollPublisher>>,
    _subscription: ScrollSubscription,
}

impl ScrollLink {
    /// Make a frame task already queued for this link publish nothing.
    fn invalidate(&self) {
        // Busy is the flush itself running; past a withdraw it gets Stale/Refused and forgets.
        if let Ok(mut publisher) = self.publisher.try_borrow_mut() {
            publisher.generation = None;
        }
    }
}

/// The retained widget of a [`NativeScaffoldView`]: the body, the refusal
/// line when refused, the fallback bars when drawn, and the chrome
/// publication it owns.
pub struct ScaffoldWidget {
    body: ChildPod,
    banner: Option<ChildPod>,
    /// What [`Self::banner`] shows, while it is up.
    banner_kind: Option<Banner>,
    /// The toolbar ids the last sync dropped, so a repeat is logged once.
    repeats: Vec<ActionId>,
    top: Option<ChildPod>,
    /// The fallback toolbar row, between the body and the tab row.
    toolbar: Option<ChildPod>,
    bottom: Option<ChildPod>,
    generation: Option<ChromeGeneration>,
    scroll: Option<ScrollLink>,
    fade: Option<ResolvedFade>,
    /// Whether the body shrinks by the keyboard inset.
    resize: bool,
    /// Whether the last layout hid the fallback tab row under the keyboard.
    bottom_hidden: bool,
    /// Whether a native arm holds the bars (not the fallback, not refused).
    native: bool,
}

/// Whether `outcome` leaves the bars to frust: always without an arm, and on
/// a refused install with one. A refused publisher holds no chrome at all.
fn draws_fallback(outcome: &PublishOutcome) -> bool {
    match outcome {
        PublishOutcome::Refused(_) | PublishOutcome::Stale => false,
        PublishOutcome::Fallback(..) => true,
        _ => !chrome::native_arm(),
    }
}

type Retire = fn(&mut ChildPod, &mut BuildCtx<'_>);

/// Bring `element`'s banner in line with `wanted`, rebuilding in place while
/// the kind holds and replacing it when the kind changes.
fn sync_banner<Outer: 'static>(
    element: &mut ScaffoldWidget,
    wanted: Option<Banner>,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    if let (Some(kind), Some(pod)) = (wanted, element.banner.as_mut())
        && element.banner_kind == Some(kind)
    {
        return rebuild_child(&kind.view::<Outer>(), &kind.view::<Outer>(), pod, ctx);
    }
    let mut flags = ChangeFlags::NONE;
    if let Some(mut pod) = element.banner.take() {
        let shown = element.banner_kind.unwrap_or(Banner::Refused);
        teardown_child(&shown.view::<Outer>(), &mut pod, ctx);
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    element.banner_kind = wanted;
    if let Some(kind) = wanted {
        element.banner = Some(build_child(&kind.view::<Outer>(), ctx));
        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
    }
    flags
}

/// Bring one fallback bar's pod in line with `view` (`None`: no bar).
fn sync_bar<Outer: 'static>(
    slot: &mut Option<ChildPod>,
    view: Option<AnyView<Outer>>,
    retire: Retire,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    match (slot.is_some(), view) {
        (false, None) => ChangeFlags::NONE,
        (false, Some(view)) => {
            *slot = Some(build_child(&view, ctx));
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
        (true, Some(view)) => match slot.as_mut() {
            Some(pod) => rebuild_child(&view, &view, pod, ctx),
            None => ChangeFlags::NONE,
        },
        (true, None) => {
            if let Some(mut pod) = slot.take() {
                retire(&mut pod, ctx);
            }
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
    }
}

impl<V> NativeScaffoldView<V> {
    /// Publish this builder's spec for `element` and bring its banner,
    /// fallback bars and scroll subscription in line with the outcome.
    fn sync<Outer: 'static>(
        &self,
        element: &mut ScaffoldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> (ChangeFlags, PublishOutcome) {
        let (spec, repeats) = self.spec_and_repeats();
        if repeats != element.repeats {
            if !repeats.is_empty() {
                let ids: Vec<&str> = repeats.iter().map(ActionId::as_str).collect();
                log::warn!(
                    "frust-native-widgets: native_scaffold toolbar entries {ids:?} repeat an \
                     action id and are not shown (ids are unique across the top bar and toolbar)"
                );
            }
            element.repeats = repeats;
        }
        let mut outcome = chrome::publish(element.generation, spec.clone());
        if outcome == PublishOutcome::Stale {
            // The generation was withdrawn under this scaffold: claim afresh.
            element.generation = None;
            outcome = chrome::publish(None, spec.clone());
        }
        let generation = outcome.generation();
        element.generation = generation;
        if let Some(generation) = generation {
            chrome::set_sink(generation, on_chrome(self.handlers.clone()));
        }
        let wanted = if generation.is_none() {
            Some(Banner::Refused)
        } else if !element.repeats.is_empty() {
            Some(Banner::Repeated)
        } else {
            None
        };
        let mut flags = sync_banner::<Outer>(element, wanted, ctx);
        let drawn = generation.filter(|_| draws_fallback(&outcome));
        let top = drawn
            .filter(|_| spec.top.is_some())
            .map(|generation| fallback_top_bar::<Outer>(&spec, generation));
        let tabs = drawn
            .filter(|_| !spec.tabs.is_empty())
            .map(|generation| fallback_tab_row::<Outer>(&spec, generation));
        let toolbar = drawn
            .filter(|_| shown_toolbar(&spec).is_some())
            .map(|generation| fallback_toolbar_row::<Outer>(&spec, generation));
        flags |= sync_bar(&mut element.top, top, teardown_top_bar::<Outer>, ctx);
        flags |= sync_bar(&mut element.bottom, tabs, teardown_tab_row::<Outer>, ctx);
        let teardown_toolbar = teardown_toolbar_row::<Outer>;
        flags |= sync_bar(&mut element.toolbar, toolbar, teardown_toolbar, ctx);
        let native = generation.is_some() && !draws_fallback(&outcome);
        if element.native != native || element.fade != self.fade {
            flags |= ChangeFlags::PAINT;
        }
        if element.resize != self.resize_to_avoid_keyboard {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.resize = self.resize_to_avoid_keyboard;
        element.native = native;
        element.fade = self.fade;
        self.sync_scroll(element, generation, spec);
        (flags, outcome)
    }

    /// (Re)subscribe to the scroll controller, if any, seeding the publisher
    /// with what `sync` just drew.
    fn sync_scroll(
        &self,
        element: &mut ScaffoldWidget,
        generation: Option<ChromeGeneration>,
        spec: ChromeSpec,
    ) {
        let native = element.native;
        let Some(controller) = &self.scroll else {
            // A flush queued before `.scroll(..)` went away must publish nothing.
            if let Some(link) = element.scroll.take() {
                link.invalidate();
            }
            return;
        };
        let publisher = match element.scroll.take() {
            Some(link) => link.publisher,
            None => Rc::new(RefCell::new(ScrollPublisher::new(
                generation,
                spec.clone(),
                native,
            ))),
        };
        publisher.borrow_mut().adopt(generation, spec, native);
        let listener = Rc::clone(&publisher);
        // The controller publishes its extent before notifying, so the
        // viewport is read live: the rebuild-time value is 0 before layout.
        let source = controller.clone();
        // `ScrollController` has no identity to compare, so every rebuild
        // resubscribes; the publisher (and its coalescing flag) carries over.
        let subscription = controller.on_change(move |info| {
            let viewport = source.viewport_extent();
            let mut publisher = listener.borrow_mut();
            if publisher.observe(info, viewport) {
                let schedule = publisher.schedule;
                drop(publisher);
                schedule(Rc::clone(&listener));
            }
        });
        element.scroll = Some(ScrollLink {
            publisher,
            _subscription: subscription,
        });
    }

    fn build_synced<Outer: 'static>(
        &self,
        ctx: &mut BuildCtx<'_>,
    ) -> (ScaffoldWidget, PublishOutcome)
    where
        V: View<Outer>,
    {
        let mut element = ScaffoldWidget {
            body: build_child(&self.body, ctx),
            banner: None,
            banner_kind: None,
            repeats: Vec::new(),
            top: None,
            toolbar: None,
            bottom: None,
            generation: None,
            scroll: None,
            fade: None,
            resize: self.resize_to_avoid_keyboard,
            bottom_hidden: false,
            native: false,
        };
        let (_, outcome) = self.sync::<Outer>(&mut element, ctx);
        (element, outcome)
    }

    fn rebuild_synced<Outer: 'static>(
        &self,
        prev: &Self,
        element: &mut ScaffoldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> (ChangeFlags, PublishOutcome)
    where
        V: View<Outer>,
    {
        let mut flags = rebuild_child(&prev.body, &self.body, &mut element.body, ctx);
        let (synced, outcome) = self.sync::<Outer>(element, ctx);
        flags |= synced;
        (flags, outcome)
    }
}

impl<Outer: 'static, V: View<Outer>> View<Outer> for NativeScaffoldView<V> {
    type Element = ScaffoldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScaffoldWidget {
        self.build_synced::<Outer>(ctx).0
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScaffoldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        self.rebuild_synced::<Outer>(prev, element, ctx).0
    }

    fn teardown(&self, element: &mut ScaffoldWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.body, &mut element.body, ctx);
        sync_banner::<Outer>(element, None, ctx);
        if let Some(mut top) = element.top.take() {
            teardown_top_bar::<Outer>(&mut top, ctx);
        }
        if let Some(mut toolbar) = element.toolbar.take() {
            teardown_toolbar_row::<Outer>(&mut toolbar, ctx);
        }
        if let Some(mut bottom) = element.bottom.take() {
            teardown_tab_row::<Outer>(&mut bottom, ctx);
        }
        if let Some(link) = element.scroll.take() {
            link.invalidate();
        }
        if let Some(generation) = element.generation.take() {
            chrome::withdraw(generation);
        }
    }
}

/// Frees a generation still held when the tree is dropped without
/// `View::teardown` (an Android activity recreate, a hot-patch witness
/// mismatch). After teardown it is a no-op; `withdraw` is generation-stamped,
/// so a stale element never frees a newer one. It may run the host's
/// `dispose` from `Drop`, which every arm must tolerate (Android drops on the
/// UI thread).
impl Drop for ScaffoldWidget {
    fn drop(&mut self) {
        if let Some(link) = &self.scroll {
            link.invalidate();
        }
        if let Some(generation) = self.generation.take() {
            chrome::withdraw(generation);
        }
    }
}

impl ScaffoldWidget {
    fn has_bars(&self) -> bool {
        self.top.is_some() || self.toolbar.is_some() || self.bottom.is_some()
    }

    /// Fill the edge fade bands over the body for bar insets `top`/`bottom`,
    /// when the bars float, a native arm holds them and a colour is set.
    fn paint_edge_fade(
        &self,
        scene: &mut dyn PaintScene,
        floats: bool,
        origin: Point,
        size: Size,
        (top, bottom): (f64, f64),
    ) {
        let Some(fade) = edge_fade_color(floats, self.native, self.fade) else {
            return;
        };
        let bands = edge_fade_bands(size, top, bottom, fade.tail);
        let alphas = (fade.edge_alpha, fade.bar_alpha);
        for (index, band) in bands.into_iter().enumerate() {
            if let Some((offset, band_size, solid_until)) = band {
                let at = Point::new(origin.x + offset.x, origin.y + offset.y);
                let brush =
                    edge_fade_brush(at, band_size, solid_until, index == 0, fade.color, alphas);
                scene.fill_rect_brush(at, band_size, &brush);
            }
        }
    }

    /// The keyboard height the body is shrunk by: zero when not resizing.
    fn keyboard(&self, insets: WindowInsets) -> f64 {
        if self.resize {
            insets.view_insets.bottom.max(0.0)
        } else {
            0.0
        }
    }

    /// The insets the body sees: the edges the fallback bars took, consumed;
    /// when shrunk by the keyboard, no IME inset and no bottom padding the
    /// keyboard already covers.
    fn body_insets(&self, insets: WindowInsets) -> WindowInsets {
        let below = self.bottom.is_some() || self.toolbar.is_some();
        let mut body = insets.consuming(false, self.top.is_some(), false, below);
        let ime = self.keyboard(insets);
        if ime > 0.0 {
            body.view_insets.bottom = 0.0;
            body.view_padding.bottom = (body.view_padding.bottom - ime).max(0.0);
        }
        body
    }
}

/// [`frust::authoring::route_event`] over the scaffold's separately held
/// pods, given in paint order.
fn route_pods(
    pods: &mut [&mut ChildPod],
    ctx: &mut EventCtx<'_>,
    event: &InputEvent,
) -> EventResult {
    if event.is_broadcast() {
        for pod in pods.iter_mut() {
            pod.event_child(ctx, event);
        }
        return EventResult::Ignored;
    }
    if event.is_focus_routed() {
        return match pods.iter_mut().find(|pod| pod.holds_live_focus()) {
            Some(pod) => pod.event_child(ctx, event),
            None => EventResult::Ignored,
        };
    }
    if let Some(pod) = pods.iter_mut().find(|pod| pod.is_active()) {
        return route_event_single(pod, ctx, event);
    }
    let position = event.position();
    let mut handled = EventResult::Ignored;
    let mut kept_focus = None;
    for (i, pod) in pods.iter_mut().enumerate().rev() {
        if pod.contains(position) && pod.event_child(ctx, event) == EventResult::Handled {
            kept_focus = pod.holds_live_focus().then_some(i);
            handled = EventResult::Handled;
            break;
        }
    }
    if matches!(event, InputEvent::Pointer(p) if p.phase == PointerPhase::Down) {
        for (i, pod) in pods.iter_mut().enumerate() {
            if Some(i) != kept_focus && pod.is_focused() {
                pod.set_focused(false);
            }
        }
    }
    handled
}

impl Widget for ScaffoldWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let banner_height = match &mut self.banner {
            Some(banner) => {
                let loose = BoxConstraints::new(
                    Size::new(bc.min().width, 0.0),
                    Size::new(bc.max().width, bc.max().height),
                );
                let height = banner.layout_child(ctx, &loose).height;
                banner.set_origin(Point::ORIGIN);
                height
            }
            None => 0.0,
        };
        let window = ctx.window_insets();
        let ime = self.keyboard(window);
        self.bottom_hidden = ime > 0.0 && self.bottom.is_some();
        if !self.has_bars() {
            let cut = banner_height + ime;
            let body_bc = BoxConstraints::new(
                Size::new(bc.min().width, (bc.min().height - cut).max(0.0)),
                Size::new(bc.max().width, (bc.max().height - cut).max(0.0)),
            );
            let insets = self.body_insets(window);
            let body_pod = &mut self.body;
            let body = ctx.with_window_insets(insets, |ctx| body_pod.layout_child(ctx, &body_bc));
            self.body.set_origin(Point::new(0.0, banner_height));
            return bc.constrain(Size::new(body.width, body.height + banner_height));
        }
        // Column: banner, top bar, body (flexible), toolbar row, tab row — bars full width.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            bc.min().width
        };
        let bar_bc = BoxConstraints::new(Size::new(width, 0.0), Size::new(width, bc.max().height));
        let mut bar_height = |bar: &mut Option<ChildPod>| {
            bar.as_mut()
                .map_or(0.0, |bar| bar.layout_child(ctx, &bar_bc).height)
        };
        let top_height = bar_height(&mut self.top);
        let bottom_height = if self.bottom_hidden {
            0.0
        } else {
            bar_height(&mut self.bottom)
        };
        // The toolbar row takes the bottom inset only when it is the lowest bar.
        let toolbar_insets = if self.bottom.is_some() && !self.bottom_hidden {
            window.consuming(false, false, false, true)
        } else {
            window
        };
        let toolbar_height = match &mut self.toolbar {
            Some(toolbar) => ctx.with_window_insets(toolbar_insets, |ctx| {
                toolbar.layout_child(ctx, &bar_bc).height
            }),
            None => 0.0,
        };
        let used = banner_height + top_height + toolbar_height + bottom_height;
        let body_bc = if bc.max().height.is_finite() {
            let height = (bc.max().height - used - ime).max(0.0);
            BoxConstraints::new(
                Size::new(bc.min().width, height),
                Size::new(bc.max().width, height),
            )
        } else {
            BoxConstraints::new(
                Size::new(bc.min().width, 0.0),
                Size::new(bc.max().width, f64::INFINITY),
            )
        };
        let insets = self.body_insets(window);
        let body = &mut self.body;
        let body_size = ctx.with_window_insets(insets, |ctx| body.layout_child(ctx, &body_bc));
        let body_y = banner_height + top_height;
        self.body.set_origin(Point::new(0.0, body_y));
        if let Some(top) = &mut self.top {
            top.set_origin(Point::new(0.0, banner_height));
        }
        let below_body = body_y + body_size.height;
        if let Some(toolbar) = &mut self.toolbar {
            toolbar.set_origin(Point::new(0.0, below_body));
        }
        if let Some(bottom) = &mut self.bottom {
            bottom.set_origin(Point::new(0.0, below_body + toolbar_height));
        }
        bc.constrain(Size::new(
            body_size.width.max(width),
            used + body_size.height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let window = ctx.window_insets();
        let insets = self.body_insets(window);
        let body = &mut self.body;
        ctx.with_window_insets(insets, |ctx| body.paint_child(ctx, scene));
        let padding = window.view_padding;
        self.paint_edge_fade(
            scene,
            BARS_FLOAT_OVER_BODY,
            ctx.origin(),
            ctx.size(),
            (padding.top, padding.bottom),
        );
        let bottom = if self.bottom_hidden {
            None
        } else {
            self.bottom.as_mut()
        };
        for pod in [
            self.top.as_mut(),
            self.toolbar.as_mut(),
            bottom,
            self.banner.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if !self.has_bars() {
            return route_event_single(&mut self.body, ctx, event);
        }
        let mut pods: Vec<&mut ChildPod> = std::iter::once(&mut self.body)
            .chain(self.top.as_mut())
            .chain(self.toolbar.as_mut())
            .chain(self.bottom.as_mut().filter(|_| !self.bottom_hidden))
            .collect();
        route_pods(&mut pods, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in [&self.top, &self.banner].into_iter().flatten() {
            pod.semantics_child(ctx);
        }
        self.body.semantics_child(ctx);
        if let Some(toolbar) = &self.toolbar {
            toolbar.semantics_child(ctx);
        }
        if let Some(bottom) = self.bottom.as_ref().filter(|_| !self.bottom_hidden) {
            bottom.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(body, banner, top, toolbar, bottom);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MenuItem, TabIcon};
    use crate::chrome::{decode_event, encode_event};
    use frust::text;
    use std::sync::Mutex;

    fn scaffold() -> NativeScaffoldView<frust::TextView> {
        native_scaffold(text("body"))
            .tabs(
                vec![
                    TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())),
                    TabItem::new("inbox", "Inbox", TabIcon::AppleSymbol("tray".into())),
                ],
                "home",
            )
            .title("Home")
            .actions(vec![
                ActionItem::new("add", "Add"),
                ActionItem::new("more", "More").menu(vec![MenuItem::new("share", "Share")]),
            ])
    }

    fn build(view: &NativeScaffoldView<frust::TextView>) -> (ScaffoldWidget, PublishOutcome) {
        let mut counter = 0u64;
        view.build_synced::<()>(&mut BuildCtx::new(&mut counter))
    }

    fn rebuild(
        prev: &NativeScaffoldView<frust::TextView>,
        next: &NativeScaffoldView<frust::TextView>,
        element: &mut ScaffoldWidget,
    ) -> PublishOutcome {
        let mut counter = 0u64;
        next.rebuild_synced::<()>(prev, element, &mut BuildCtx::new(&mut counter))
            .1
    }

    fn teardown(view: &NativeScaffoldView<frust::TextView>, element: &mut ScaffoldWidget) {
        let mut counter = 0u64;
        <NativeScaffoldView<_> as View<()>>::teardown(
            view,
            element,
            &mut BuildCtx::new(&mut counter),
        );
    }

    #[test]
    fn publishes_once_per_distinct_spec() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut element, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        assert!(matches!(
            rebuild(&view, &scaffold(), &mut element),
            PublishOutcome::Unchanged(_)
        ));
        let changed = scaffold().title("Inbox");
        assert!(matches!(
            rebuild(&view, &changed, &mut element),
            PublishOutcome::Applied(_)
        ));
        assert!(matches!(
            rebuild(&changed, &changed_again(), &mut element),
            PublishOutcome::Unchanged(_)
        ));
        teardown(&changed, &mut element);
    }

    fn changed_again() -> NativeScaffoldView<frust::TextView> {
        scaffold().title("Inbox")
    }

    #[test]
    fn teardown_withdraws_so_the_next_scaffold_installs() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut first, _) = build(&view);
        teardown(&view, &mut first);
        let (mut second, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        teardown(&view, &mut second);
    }

    #[test]
    fn a_scaffold_dropped_without_teardown_frees_the_slot() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (first, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        drop(first);
        assert_eq!(chrome::live_generation(), None);
        let (mut second, outcome) = build(&view);
        assert!(
            matches!(outcome, PublishOutcome::Installed(_)),
            "got {outcome:?}"
        );
        teardown(&view, &mut second);
    }

    #[test]
    fn dropping_a_torn_down_scaffold_is_a_no_op() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut first, _) = build(&view);
        teardown(&view, &mut first);
        let (mut second, _) = build(&view);
        assert!(second.generation.is_some());
        drop(first);
        assert_eq!(chrome::live_generation(), second.generation);
        teardown(&view, &mut second);
    }

    #[test]
    fn a_stale_scaffolds_drop_leaves_a_newer_generation_live() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (first, _) = build(&view);
        assert!(chrome::withdraw(first.generation.expect("installed")));
        let (mut second, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        drop(first);
        assert_eq!(chrome::live_generation(), second.generation);
        teardown(&view, &mut second);
    }

    #[test]
    fn a_scaffold_whose_generation_was_withdrawn_reinstalls_on_rebuild() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut element, _) = build(&view);
        let first = element.generation.expect("installed");
        assert!(chrome::withdraw(first));
        let outcome = rebuild(&view, &scaffold(), &mut element);
        let PublishOutcome::Installed(second) = outcome else {
            panic!("expected Installed, got {outcome:?}");
        };
        assert!(second > first);
        assert_eq!(element.generation, Some(second));
        assert!(element.banner.is_none());
        teardown(&view, &mut element);
    }

    #[test]
    fn a_second_live_scaffold_is_refused_and_shows_the_banner() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut first, _) = build(&view);
        let (mut second, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Refused(_)));
        assert!(second.banner.is_some() && second.generation.is_none());
        assert!(second.top.is_none() && second.bottom.is_none(), "no bars");
        assert!(first.banner.is_none());
        assert!(
            first.top.is_some() && first.bottom.is_some(),
            "fallback bars"
        );
        assert!(matches!(
            rebuild(&view, &scaffold(), &mut first),
            PublishOutcome::Unchanged(_)
        ));
        teardown(&view, &mut first);
        assert!(matches!(
            rebuild(&view, &scaffold(), &mut second),
            PublishOutcome::Installed(_)
        ));
        assert!(
            second.banner.is_none(),
            "the banner goes once it owns the slot"
        );
        teardown(&view, &mut second);
    }

    #[test]
    fn every_event_reaches_its_typed_callback_and_events_into() {
        let _guard = chrome::slot_test_guard();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let note = |tag: &'static str| {
            let log = Arc::clone(&log);
            move |detail: String| log.lock().unwrap().push(format!("{tag}:{detail}"))
        };
        let (select, reselect, action, menu, back) = (
            note("select"),
            note("reselect"),
            note("action"),
            note("menu"),
            note("back"),
        );
        let last: RwSignal<Option<ChromeEvent>> = RwSignal::new(None);
        let view = scaffold()
            .on_tab_select(move |id| select(id.to_string()))
            .on_tab_reselect(move |id| reselect(id.to_string()))
            .on_action(move |id| action(id.to_string()))
            .on_menu(move |a, m| menu(format!("{a}/{m}")))
            .on_back(move || back(String::new()))
            .events_into(last);
        let (mut element, _) = build(&view);
        let generation = element.generation.expect("installed").get();
        let spec = view.spec();
        let events = [
            ChromeEvent::TabSelected(TabId::new("inbox")),
            ChromeEvent::TabReselected(TabId::new("home")),
            ChromeEvent::ActionTapped("add".into()),
            ChromeEvent::MenuItemChosen("more".into(), "share".into()),
            ChromeEvent::BackPressed,
        ];
        for event in &events {
            let (kind, detail) = encode_event(&spec, event).expect("spec carries the ids");
            assert_eq!(decode_event(&spec, kind, detail).as_ref(), Some(event));
            assert!(chrome::deliver(generation, kind, detail));
            assert_eq!(
                frust::GetUntracked::get_untracked(&last).as_ref(),
                Some(event)
            );
        }
        assert_eq!(
            *log.lock().unwrap(),
            [
                "select:inbox",
                "reselect:home",
                "action:add",
                "menu:more/share",
                "back:"
            ]
        );
        teardown(&view, &mut element);
        assert!(
            !chrome::deliver(generation, 0, 0),
            "a withdrawn generation's reports are dropped"
        );
    }

    #[test]
    fn a_repeated_toolbar_id_is_dropped_bannered_and_cleared() {
        let _guard = chrome::slot_test_guard();
        let toolbar = ToolbarSpec::new()
            .item(ActionItem::new("add", "Add again"))
            .item(ActionItem::new("info", "Info"))
            .item(ActionItem::new("info", "Info twice"))
            .primary(ActionItem::new("comment", "Comment"));
        let view = scaffold().toolbar(toolbar);
        let (spec, repeats) = view.spec_and_repeats();
        assert_eq!(repeats, [ActionId::new("add"), ActionId::new("info")]);
        let shown = spec.toolbar.expect("a toolbar");
        let ids: Vec<&str> = shown.entries().map(|a| a.id().as_str()).collect();
        assert_eq!(ids, ["info", "comment"]);
        assert_eq!(shown.items[0].title, "Info", "the first carrier stays");

        let (mut element, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        assert_eq!(element.banner_kind, Some(Banner::Repeated));
        assert!(element.banner.is_some());
        let live = chrome::live_spec().expect("live");
        assert_eq!(live.toolbar.map(|t| t.items.len()), Some(1));

        let clean = scaffold().toolbar(
            ToolbarSpec::new()
                .item(ActionItem::new("info", "Info"))
                .primary(ActionItem::new("comment", "Comment")),
        );
        rebuild(&view, &clean, &mut element);
        assert!(element.banner.is_none() && element.banner_kind.is_none());
        assert!(element.repeats.is_empty());
        teardown(&clean, &mut element);
    }

    #[test]
    fn the_refusal_banner_outranks_a_repeat_and_swaps_back() {
        let _guard = chrome::slot_test_guard();
        let repeated = scaffold().toolbar(ToolbarSpec::new().primary(ActionItem::new("add", "A")));
        let (mut first, _) = build(&scaffold());
        let (mut second, outcome) = build(&repeated);
        assert!(matches!(outcome, PublishOutcome::Refused(_)));
        assert_eq!(second.banner_kind, Some(Banner::Refused));
        teardown(&scaffold(), &mut first);
        assert!(matches!(
            rebuild(&repeated, &repeated, &mut second),
            PublishOutcome::Installed(_)
        ));
        assert_eq!(second.banner_kind, Some(Banner::Repeated));
        teardown(&repeated, &mut second);
        assert!(second.banner.is_none());
    }

    #[test]
    fn toolbar_actions_and_menus_reach_the_shared_handlers() {
        let _guard = chrome::slot_test_guard();
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let (actions, menus) = (Arc::clone(&log), Arc::clone(&log));
        let view = scaffold()
            .toolbar(
                ToolbarSpec::new()
                    .item(
                        ActionItem::new("sort", "Sort").menu(vec![MenuItem::new("new", "Newest")]),
                    )
                    .primary(ActionItem::new("comment", "Comment")),
            )
            .on_action(move |id| actions.lock().unwrap().push(format!("action:{id}")))
            .on_menu(move |a, m| menus.lock().unwrap().push(format!("menu:{a}/{m}")));
        let (mut element, _) = build(&view);
        let generation = element.generation.expect("installed").get();
        let spec = view.spec();
        for event in [
            ChromeEvent::ActionTapped("comment".into()),
            ChromeEvent::MenuItemChosen("sort".into(), "new".into()),
        ] {
            let (kind, detail) = encode_event(&spec, &event).expect("spec carries the ids");
            assert!(chrome::deliver(generation, kind, detail));
        }
        assert_eq!(*log.lock().unwrap(), ["action:comment", "menu:sort/new"]);
        teardown(&view, &mut element);
    }

    #[test]
    fn tab_select_into_writes_the_requested_tab() {
        let _guard = chrome::slot_test_guard();
        let picked = RwSignal::new(TabId::new("home"));
        let view = scaffold().on_tab_select_into(picked);
        let (mut element, _) = build(&view);
        let (kind, detail) =
            encode_event(&view.spec(), &ChromeEvent::TabSelected(TabId::new("inbox"))).unwrap();
        chrome::deliver(element.generation.unwrap().get(), kind, detail);
        assert_eq!(
            frust::GetUntracked::get_untracked(&picked),
            TabId::new("inbox")
        );
        teardown(&view, &mut element);
    }

    #[test]
    fn the_scroll_publisher_publishes_changed_values_only_once_per_flush() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut element, _) = build(&view);
        let generation = element.generation;
        let mut publisher = ScrollPublisher::new(generation, view.spec(), element.native);
        let at = |offset| ScrollInfo {
            offset,
            max_offset: 400.0,
            overscroll: 0.0,
        };
        assert!(
            publisher.observe(at(10.0), 600.0),
            "first change schedules a flush"
        );
        assert!(
            !publisher.observe(at(20.0), 600.0),
            "a flush is already queued"
        );
        assert!(matches!(
            publisher.flush(),
            Some(PublishOutcome::Applied(_))
        ));
        assert!(
            !publisher.observe(at(20.0), 600.0),
            "an unchanged value schedules nothing"
        );
        assert!(publisher.observe(at(30.0), 600.0));
        assert!(matches!(
            publisher.flush(),
            Some(PublishOutcome::Applied(_))
        ));
        assert_eq!(
            publisher.spec.scroll,
            Some(ScrollState {
                offset: 30.0,
                max_offset: 400.0,
                viewport: 600.0
            })
        );
        teardown(&view, &mut element);
    }

    fn scrolled(offset: f64) -> ScrollInfo {
        ScrollInfo {
            offset,
            max_offset: 400.0,
            overscroll: 0.0,
        }
    }

    fn scroll_publisher(element: &ScaffoldWidget) -> Rc<RefCell<ScrollPublisher>> {
        Rc::clone(&element.scroll.as_ref().expect("subscribed").publisher)
    }

    #[test]
    fn a_scroll_flush_queued_before_teardown_publishes_nothing() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold().scroll(ScrollController::new());
        let (mut element, _) = build(&view);
        let queued = scroll_publisher(&element);
        assert!(
            queued.borrow_mut().observe(scrolled(10.0), 600.0),
            "flush queued"
        );
        teardown(&view, &mut element);
        // What the spawned frame task does at the next local pump.
        let _ = queued.borrow_mut().flush();
        assert_eq!(chrome::live_generation(), None);
        let (mut next, outcome) = build(&view);
        assert!(
            matches!(outcome, PublishOutcome::Installed(_)),
            "got {outcome:?}"
        );
        teardown(&view, &mut next);
    }

    thread_local! {
        /// Flushes [`queue_flush`] scheduled, for a test to run by hand.
        static QUEUED: RefCell<Vec<Rc<RefCell<ScrollPublisher>>>> = const { RefCell::new(Vec::new()) };
    }

    /// A [`ScheduleFlush`] that queues for [`run_queued_flushes`] instead of
    /// needing a UI-thread executor.
    fn queue_flush(publisher: Rc<RefCell<ScrollPublisher>>) {
        QUEUED.with(|queued| queued.borrow_mut().push(publisher));
    }

    fn run_queued_flushes() -> Vec<Option<PublishOutcome>> {
        let queued = QUEUED.with(|queued| std::mem::take(&mut *queued.borrow_mut()));
        queued
            .iter()
            .map(|task| task.borrow_mut().flush())
            .collect()
    }

    #[test]
    fn a_published_scroll_state_carries_the_live_viewport_after_layout() {
        let _guard = chrome::slot_test_guard();
        let controller = ScrollController::new();
        let body = frust::scroll_view(frust::colored_box().size(300.0, 2000.0))
            .controller(controller.clone());
        let view = native_scaffold(body).scroll(controller.clone());
        let mut counter = 0u64;
        let (mut element, outcome) = view.build_synced::<()>(&mut BuildCtx::new(&mut counter));
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        let built = chrome::live_spec().and_then(|spec| spec.scroll);
        assert!(
            built.is_none_or(|scroll| scroll.viewport == 0.0),
            "nothing is laid out at build: {built:?}"
        );
        scroll_publisher(&element).borrow_mut().schedule = queue_flush;
        let bc = BoxConstraints::loose(Size::new(300.0, 500.0));
        element.layout(&mut LayoutCtx::new(), &bc);
        let outcomes = run_queued_flushes();
        assert!(
            matches!(outcomes.as_slice(), [Some(PublishOutcome::Applied(_))]),
            "one coalesced publish: {outcomes:?}"
        );
        assert_eq!(controller.viewport_extent(), 500.0);
        assert_eq!(
            chrome::live_spec().and_then(|spec| spec.scroll),
            Some(ScrollState {
                offset: 0.0,
                max_offset: 1500.0,
                viewport: 500.0,
            })
        );
        let mut counter = 0u64;
        View::<()>::teardown(&view, &mut element, &mut BuildCtx::new(&mut counter));
    }

    #[test]
    fn removing_scroll_invalidates_a_flush_queued_before() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold().scroll(ScrollController::new());
        let (mut element, _) = build(&view);
        let queued = scroll_publisher(&element);
        assert!(
            queued.borrow_mut().observe(scrolled(10.0), 600.0),
            "flush queued"
        );
        let unscrolled = scaffold();
        assert!(matches!(
            rebuild(&view, &unscrolled, &mut element),
            PublishOutcome::Unchanged(_)
        ));
        assert!(element.scroll.is_none());
        assert_eq!(queued.borrow_mut().flush(), None, "publishes nothing");
        assert_eq!(
            chrome::live_spec().map(|spec| spec.scroll),
            Some(None),
            "no stale scroll state reached the slot"
        );
        teardown(&unscrolled, &mut element);
    }

    #[test]
    fn teardown_invalidates_the_scroll_publisher() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold().scroll(ScrollController::new());
        let (mut element, _) = build(&view);
        let publisher = scroll_publisher(&element);
        assert!(publisher.borrow().generation.is_some());
        teardown(&view, &mut element);
        assert_eq!(publisher.borrow_mut().flush(), None);
        assert_eq!(chrome::live_generation(), None);
    }

    #[test]
    fn a_flush_that_no_longer_holds_the_slot_forgets_its_generation() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut first, _) = build(&view);
        let withdrawn = first.generation.expect("installed");
        assert!(chrome::withdraw(withdrawn));
        let (mut second, _) = build(&view);
        let flushed = || {
            let mut publisher = ScrollPublisher::new(Some(withdrawn), view.spec(), false);
            publisher.observe(scrolled(10.0), 600.0);
            let outcome = publisher.flush().expect("held a generation");
            (outcome, publisher.generation)
        };
        let (outcome, kept) = flushed();
        assert_eq!(outcome.generation(), None, "another is live: {outcome:?}");
        assert_eq!(kept, None);
        assert_eq!(chrome::live_generation(), second.generation);
        teardown(&view, &mut second);
        let (outcome, kept) = flushed();
        assert_eq!(outcome.generation(), None, "slot empty: {outcome:?}");
        assert_eq!(kept, None);
        assert_eq!(chrome::live_generation(), None);
        teardown(&view, &mut first);
    }

    /// Observe and flush one scroll step through `element`'s publisher,
    /// answering the outcome and how many epoch bumps it made.
    fn flush_scroll(element: &ScaffoldWidget, offset: f64) -> (PublishOutcome, u64) {
        let publisher = scroll_publisher(element);
        let before = chrome::epoch();
        publisher.borrow_mut().observe(scrolled(offset), 600.0);
        let outcome = publisher.borrow_mut().flush().expect("holds a generation");
        (outcome, chrome::epoch() - before)
    }

    fn scrolling() -> NativeScaffoldView<frust::TextView> {
        scaffold().scroll(ScrollController::new())
    }

    fn fallback_pods(element: &ScaffoldWidget) -> bool {
        element.top.is_some() && element.bottom.is_some()
    }

    #[test]
    fn a_flush_into_the_fallback_bumps_once_and_the_next_sync_draws_it() {
        let _guard = chrome::slot_test_guard();
        let _arm = chrome::ArmOverride::new(true);
        let view = scrolling();
        let (mut element, outcome) = build(&view);
        let generation = outcome.generation().expect("installed");
        assert!(element.native && !fallback_pods(&element));
        chrome::with_test_host(|host| {
            host.fail_deferred(generation, chrome::ChromeError::NoHost);
            host.set_refusal(Some(chrome::ChromeError::NoHost));
        });
        let (outcome, bumps) = flush_scroll(&element, 10.0);
        assert!(
            matches!(outcome, PublishOutcome::Fallback(..)),
            "{outcome:?}"
        );
        assert_eq!(bumps, 1, "the crossing bumps the epoch once");
        let (outcome, bumps) = flush_scroll(&element, 20.0);
        assert!(
            matches!(outcome, PublishOutcome::Fallback(..)),
            "{outcome:?}"
        );
        assert_eq!(bumps, 0, "still fallback: no second bump");
        assert!(element.native, "nothing re-synced yet");
        let outcome = rebuild(&view, &scrolling(), &mut element);
        assert!(
            matches!(outcome, PublishOutcome::Fallback(..)),
            "{outcome:?}"
        );
        assert!(
            !element.native && fallback_pods(&element),
            "the sync drew the fallback"
        );
        teardown(&view, &mut element);
    }

    #[test]
    fn a_flush_that_installs_bumps_once_and_the_next_sync_drops_the_fallback() {
        let _guard = chrome::slot_test_guard();
        let _arm = chrome::ArmOverride::new(true);
        chrome::with_test_host(|host| host.set_refusal(Some(chrome::ChromeError::NoHost)));
        let view = scrolling();
        let (mut element, outcome) = build(&view);
        assert!(
            matches!(outcome, PublishOutcome::Fallback(..)),
            "{outcome:?}"
        );
        assert!(!element.native && fallback_pods(&element));
        let (outcome, bumps) = flush_scroll(&element, 10.0);
        assert!(
            matches!(outcome, PublishOutcome::Fallback(..)),
            "{outcome:?}"
        );
        assert_eq!(bumps, 0, "a NoHost retry that fails again changes nothing");
        chrome::with_test_host(|host| host.set_refusal(None));
        let (outcome, bumps) = flush_scroll(&element, 20.0);
        assert!(
            matches!(outcome, PublishOutcome::Installed(_)),
            "{outcome:?}"
        );
        assert_eq!(bumps, 1);
        for offset in [30.0, 30.0, 40.0] {
            let (outcome, bumps) = flush_scroll(&element, offset);
            assert!(
                matches!(
                    outcome,
                    PublishOutcome::Applied(_) | PublishOutcome::Unchanged(_)
                ),
                "{outcome:?}"
            );
            assert_eq!(bumps, 0, "Applied/Unchanged never bump");
        }
        assert!(fallback_pods(&element), "nothing re-synced yet");
        rebuild(&view, &scrolling(), &mut element);
        assert!(
            element.native && !fallback_pods(&element),
            "the sync dropped the fallback"
        );
        teardown(&view, &mut element);
    }

    #[test]
    fn a_reported_failure_never_leaves_the_scaffold_native_after_sync() {
        let _guard = chrome::slot_test_guard();
        let _arm = chrome::ArmOverride::new(true);
        for error in [
            chrome::ChromeError::NoHost,
            chrome::ChromeError::Refused("a native chrome step panicked".into()),
        ] {
            let view = scaffold();
            let (mut element, outcome) = build(&view);
            let generation = outcome.generation().expect("installed");
            assert!(element.native);
            chrome::with_test_host(|host| host.fail_deferred(generation, error.clone()));
            let outcome = rebuild(&view, &scaffold(), &mut element);
            assert_eq!(outcome, PublishOutcome::Fallback(generation, error.clone()));
            assert!(!element.native && fallback_pods(&element), "{error:?}");
            let outcome = rebuild(&view, &scaffold(), &mut element);
            let retried = error == chrome::ChromeError::NoHost;
            assert_eq!(
                matches!(outcome, PublishOutcome::Installed(_)),
                retried,
                "{outcome:?}"
            );
            assert_eq!(element.native, retried);
            assert_eq!(fallback_pods(&element), !retried);
            teardown(&view, &mut element);
        }
    }

    #[test]
    fn the_spec_carries_the_ambient_theme_brightness() {
        use frust::Brightness;
        use std::cell::Cell;

        thread_local! {
            static SEEN: Cell<Option<bool>> = const { Cell::new(None) };
        }
        struct Themed(Option<Brightness>);
        impl frust::Component for Themed {
            type State = ();
            fn init(&self) {
                if let Some(brightness) = self.0 {
                    frust::provide_context(Theme::neutral().with_brightness(brightness));
                }
            }
            fn build(&self, _: &mut ()) -> impl View<()> {
                SEEN.with(|seen| seen.set(Some(scaffold().spec().dark)));
                text("body")
            }
        }
        let themes = [
            (Some(Brightness::Dark), true),
            (Some(Brightness::Light), false),
            (None, false),
        ];
        for (brightness, dark) in themes {
            let mut counter = 0u64;
            let view = frust::component(Themed(brightness));
            let _ = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
            assert_eq!(SEEN.with(Cell::take), Some(dark), "{brightness:?}");
        }
        assert!(!scaffold().spec().dark, "no owner, no theme: light");
    }

    #[test]
    fn the_fallback_draws_unless_refused_and_follows_the_spec() {
        let _guard = chrome::slot_test_guard();
        let generation = || chrome::publish(None, ChromeSpec::default()).generation();
        let held = generation().expect("an empty slot installs");
        let refused = chrome::ChromeError::Refused("stock theme".into());
        assert!(draws_fallback(&PublishOutcome::Fallback(held, refused)));
        assert!(!draws_fallback(&PublishOutcome::Refused(
            chrome::ChromeError::Busy
        )));
        assert!(!draws_fallback(&PublishOutcome::Stale));
        assert_eq!(
            draws_fallback(&PublishOutcome::Installed(held)),
            !chrome::NATIVE_CHROME_ARM
        );
        chrome::withdraw(held);
        let view = native_scaffold(text("body")).title("Only a title");
        let (mut element, _) = build(&view);
        assert!(element.top.is_some() && element.bottom.is_none());
        let tabs_only = native_scaffold(text("body")).tabs(
            vec![TabItem::new("a", "A", TabIcon::AppleSymbol("a".into()))],
            "a",
        );
        rebuild(&view, &tabs_only, &mut element);
        assert!(element.top.is_none() && element.bottom.is_some());
        teardown(&tabs_only, &mut element);
    }

    #[test]
    fn layout_stacks_the_banner_above_the_body() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut first, _) = build(&view);
        let (mut second, _) = build(&view);
        let bc = BoxConstraints::loose(Size::new(300.0, 500.0));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn std::any::Any);
        first.layout(&mut lctx, &bc);
        second.layout(&mut lctx, &bc);
        assert!(
            second.body.origin().y > 0.0,
            "the banner sits above the body"
        );
        assert_eq!(
            first.body.origin().y,
            first.top.as_ref().map_or(0.0, |top| top.size().height),
            "the fallback top bar sits above the body"
        );
        teardown(&view, &mut first);
        teardown(&view, &mut second);
    }

    /// What the probe body saw on its last layout.
    #[derive(Default)]
    struct Seen {
        max: Size,
        insets: WindowInsets,
    }

    struct ProbeBody(Rc<RefCell<Seen>>);

    impl Widget for ProbeBody {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            *self.0.borrow_mut() = Seen {
                max: bc.max(),
                insets: ctx.window_insets(),
            };
            bc.max()
        }
        fn paint(&mut self, _: &mut PaintCtx, _: &mut dyn PaintScene) {}
    }

    /// Lay `view` out in 300x800 under `insets`, with a probe as the body.
    fn laid_out(
        view: &NativeScaffoldView<frust::TextView>,
        insets: WindowInsets,
    ) -> (ScaffoldWidget, Seen) {
        let (mut element, _) = build(view);
        let seen = Rc::new(RefCell::new(Seen::default()));
        element.body = ChildPod::new(Box::new(ProbeBody(seen.clone())));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn std::any::Any);
        let bc = BoxConstraints::loose(Size::new(300.0, 800.0));
        lctx.with_window_insets(insets, |ctx| element.layout(ctx, &bc));
        let seen = std::mem::take(&mut *seen.borrow_mut());
        (element, seen)
    }

    fn keyboard_insets(padding_bottom: f64, ime: f64) -> WindowInsets {
        WindowInsets::new(
            frust_core::insets::EdgeInsets::new(0.0, 0.0, 0.0, padding_bottom),
            frust_core::insets::EdgeInsets::new(0.0, 0.0, 0.0, ime),
        )
    }

    fn bare() -> NativeScaffoldView<frust::TextView> {
        native_scaffold(text("body"))
    }

    #[test]
    fn resizing_to_avoid_the_keyboard_is_the_default() {
        assert!(bare().resize_to_avoid_keyboard);
        assert!(
            !bare()
                .resize_to_avoid_keyboard(false)
                .resize_to_avoid_keyboard
        );
    }

    #[test]
    fn the_body_shrinks_by_the_keyboard_without_bars() {
        let _guard = chrome::slot_test_guard();
        let view = bare();
        let (mut element, seen) = laid_out(&view, keyboard_insets(34.0, 300.0));
        assert_eq!(seen.max.height, 500.0);
        assert_eq!(element.body.origin().y, 0.0);
        assert_eq!(seen.insets.view_insets.bottom, 0.0);
        assert_eq!(seen.insets.view_padding.bottom, 0.0);
        teardown(&view, &mut element);
    }

    #[test]
    fn the_body_is_unchanged_when_resizing_is_disabled() {
        let _guard = chrome::slot_test_guard();
        let insets = keyboard_insets(34.0, 300.0);
        for view in [
            bare().resize_to_avoid_keyboard(false),
            scaffold().resize_to_avoid_keyboard(false),
        ] {
            let (mut element, seen) = laid_out(&view, insets);
            assert_eq!(seen.insets.view_insets.bottom, 300.0);
            assert!(!element.bottom_hidden);
            if !element.has_bars() {
                assert_eq!(seen.max.height, 800.0);
            }
            teardown(&view, &mut element);
        }
    }

    #[test]
    fn the_body_sees_the_bottom_padding_the_keyboard_does_not_cover() {
        let _guard = chrome::slot_test_guard();
        // Raw view_padding 340 over a 300 keyboard: 40 remains visible.
        let view = bare();
        let (mut element, seen) = laid_out(&view, keyboard_insets(340.0, 300.0));
        assert_eq!(seen.insets.view_padding.bottom, 40.0);
        assert_eq!(seen.insets.view_insets.bottom, 0.0);
        teardown(&view, &mut element);
    }

    #[test]
    fn the_fallback_bars_path_shrinks_the_body_and_hides_the_tab_row() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let (mut resting, rest) = laid_out(&view, keyboard_insets(0.0, 0.0));
        let tab_row = resting
            .bottom
            .as_ref()
            .expect("fallback tab row")
            .size()
            .height;
        let top = resting
            .top
            .as_ref()
            .expect("fallback top bar")
            .size()
            .height;
        assert!(!resting.bottom_hidden);
        assert_eq!(rest.max.height, 800.0 - top - tab_row);
        teardown(&view, &mut resting);

        let (mut up, seen) = laid_out(&view, keyboard_insets(0.0, 300.0));
        assert!(up.bottom_hidden, "the tab row hides under the keyboard");
        assert_eq!(seen.max.height, 800.0 - top - 300.0);
        assert_eq!(seen.insets.view_insets.bottom, 0.0);
        assert_eq!(up.body.origin().y, top);
        teardown(&view, &mut up);
    }

    #[test]
    fn a_keyboard_taller_than_the_space_clamps_the_body_to_zero() {
        let _guard = chrome::slot_test_guard();
        for view in [bare(), scaffold()] {
            let (mut element, seen) = laid_out(&view, keyboard_insets(0.0, 5000.0));
            assert_eq!(seen.max.height, 0.0);
            teardown(&view, &mut element);
        }
    }

    #[test]
    fn toggling_resize_requests_layout() {
        let _guard = chrome::slot_test_guard();
        let on = bare();
        let (mut element, _) = build(&on);
        let off = bare().resize_to_avoid_keyboard(false);
        let mut counter = 0u64;
        let (flags, _) =
            off.rebuild_synced::<()>(&on, &mut element, &mut BuildCtx::new(&mut counter));
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(!element.resize);
        teardown(&off, &mut element);
    }

    /// Counts and keeps every brush fill.
    #[derive(Default)]
    struct FadeScene(Vec<(Point, Size, Brush)>);

    impl PaintScene for FadeScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rect_brush(&mut self, origin: Point, size: Size, brush: &Brush) {
            self.0.push((origin, size, brush.clone()));
        }
    }

    fn paint_fade(element: &ScaffoldWidget, floats: bool) -> FadeScene {
        let mut scene = FadeScene::default();
        element.paint_edge_fade(
            &mut scene,
            floats,
            Point::ORIGIN,
            Size::new(375.0, 667.0),
            (74.0, 83.0),
        );
        scene
    }

    /// An element as `sync` leaves it, with `native` forced.
    fn faded(view: &NativeScaffoldView<frust::TextView>, native: bool) -> ScaffoldWidget {
        let (mut element, _) = build(view);
        element.native = native;
        element
    }

    #[test]
    fn the_edge_bands_span_the_inset_plus_the_tail() {
        let [top, bottom] = edge_fade_bands(Size::new(375.0, 667.0), 74.0, 83.0, EDGE_FADE_TAIL);
        assert_eq!(
            top,
            Some((Point::new(0.0, 0.0), Size::new(375.0, 98.0), 74.0 / 98.0))
        );
        assert_eq!(
            bottom,
            Some((
                Point::new(0.0, 560.0),
                Size::new(375.0, 107.0),
                83.0 / 107.0
            ))
        );
        let [top, bottom] = edge_fade_bands(Size::new(375.0, 667.0), 0.0, 83.0, EDGE_FADE_TAIL);
        assert!(top.is_none() && bottom.is_some());
        assert_eq!(
            edge_fade_bands(Size::new(375.0, 667.0), 0.0, 0.0, EDGE_FADE_TAIL),
            [None, None]
        );
    }

    #[test]
    fn the_edge_fade_paints_two_brush_fills_only_when_native_and_floating() {
        let _guard = chrome::slot_test_guard();
        let view = scaffold();
        let mut element = faded(&view, true);
        let scene = paint_fade(&element, true);
        assert_eq!(scene.0.len(), 2);
        assert_eq!(scene.0[0].0, Point::new(0.0, 0.0));
        assert_eq!(scene.0[1].0, Point::new(0.0, 560.0));
        assert!(
            paint_fade(&element, false).0.is_empty(),
            "bars beside the body"
        );
        element.native = false;
        assert!(
            paint_fade(&element, true).0.is_empty(),
            "fallback or refused"
        );
        teardown(&view, &mut element);
    }

    #[test]
    fn no_edge_fade_paints_nothing_and_edge_fade_sets_the_colour() {
        let _guard = chrome::slot_test_guard();
        let off = scaffold().no_edge_fade();
        let mut element = faded(&off, true);
        assert!(paint_fade(&element, true).0.is_empty());
        teardown(&off, &mut element);

        let red = Color::from_rgb8(200, 0, 0);
        let on = scaffold().edge_fade(EdgeFade::default().color(red));
        let mut element = faded(&on, true);
        assert_eq!(element.fade.map(|fade| fade.color), Some(red));
        let scene = paint_fade(&element, true);
        let Brush::Gradient(gradient) = &scene.0[0].2 else {
            panic!("a gradient brush");
        };
        assert_eq!(gradient.stops.len(), 3);
        assert_eq!(
            gradient.stops[0]
                .color
                .to_alpha_color::<peniko::color::Srgb>(),
            red.multiply_alpha(EDGE_FADE_EDGE_ALPHA)
        );
        teardown(&on, &mut element);
    }

    #[test]
    fn the_default_fade_is_the_theme_surface_else_white_or_black() {
        use frust::Brightness;
        use std::cell::Cell;

        thread_local! {
            static SEEN: Cell<Option<Option<Color>>> = const { Cell::new(None) };
        }
        struct Themed(Option<Brightness>);
        impl frust::Component for Themed {
            type State = ();
            fn init(&self) {
                if let Some(brightness) = self.0 {
                    frust::provide_context(Theme::neutral().with_brightness(brightness));
                }
            }
            fn build(&self, _: &mut ()) -> impl View<()> {
                SEEN.with(|seen| seen.set(Some(scaffold().fade.map(|fade| fade.color))));
                text("body")
            }
        }
        for brightness in [Brightness::Dark, Brightness::Light] {
            let mut counter = 0u64;
            let view = frust::component(Themed(Some(brightness)));
            let _ = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
            let surface = Theme::neutral()
                .with_brightness(brightness)
                .scheme()
                .surface;
            assert_eq!(SEEN.with(Cell::take), Some(Some(surface)), "{brightness:?}");
        }
        let plain = scaffold().fade.map(|fade| fade.color);
        assert_eq!(plain, Some(Color::WHITE), "no theme");
    }

    #[test]
    fn edge_fade_defaults_and_strong_preset() {
        let default = EdgeFade::default();
        assert_eq!(
            (
                default.color,
                default.edge_alpha,
                default.bar_alpha,
                default.tail
            ),
            (None, 0.92, 0.6, 24.0)
        );
        let strong = EdgeFade::strong();
        assert_eq!(
            (
                strong.color,
                strong.edge_alpha,
                strong.bar_alpha,
                strong.tail
            ),
            (None, 1.0, 0.88, 24.0)
        );
    }

    #[test]
    fn edge_fade_setters_sanitise() {
        let fade = EdgeFade::default().edge_alpha(1.5).bar_alpha(-0.2);
        assert_eq!((fade.edge_alpha, fade.bar_alpha), (1.0, 0.0));
        let fade = EdgeFade::strong().edge_alpha(f32::NAN).bar_alpha(f32::NAN);
        assert_eq!((fade.edge_alpha, fade.bar_alpha), (0.92, 0.6));
        assert_eq!(EdgeFade::default().tail(-5.0).tail, 0.0);
        assert_eq!(EdgeFade::default().tail(f64::NAN).tail, 0.0);
        assert_eq!(EdgeFade::default().tail(f64::INFINITY).tail, 0.0);
        assert_eq!(EdgeFade::default().tail(10.0).tail, 10.0);
    }

    #[test]
    fn a_custom_fade_reaches_the_stops_and_band_rects() {
        let _guard = chrome::slot_test_guard();
        let red = Color::from_rgb8(200, 0, 0);
        let view = scaffold().edge_fade(
            EdgeFade::default()
                .color(red)
                .edge_alpha(0.5)
                .bar_alpha(0.25)
                .tail(10.0),
        );
        let mut element = faded(&view, true);
        let scene = paint_fade(&element, true);
        assert_eq!(scene.0[0].1, Size::new(375.0, 84.0));
        assert_eq!(scene.0[1].0, Point::new(0.0, 574.0));
        let Brush::Gradient(gradient) = &scene.0[0].2 else {
            panic!("a gradient brush");
        };
        let at = |index: usize| {
            gradient.stops[index]
                .color
                .to_alpha_color::<peniko::color::Srgb>()
        };
        assert_eq!(at(0), red.multiply_alpha(0.5));
        assert_eq!(at(1), red.multiply_alpha(0.25));
        teardown(&view, &mut element);
    }

    #[test]
    fn the_fade_colour_overrides_or_resolves_to_the_theme_surface() {
        use std::cell::RefCell;

        thread_local! {
            static SEEN: RefCell<Vec<Option<Color>>> = const { RefCell::new(Vec::new()) };
        }
        let red = Color::from_rgb8(200, 0, 0);
        struct Themed;
        impl frust::Component for Themed {
            type State = ();
            fn init(&self) {
                frust::provide_context(Theme::neutral());
            }
            fn build(&self, _: &mut ()) -> impl View<()> {
                let seen = |fade: EdgeFade| {
                    scaffold()
                        .edge_fade(fade)
                        .fade
                        .map(|resolved| resolved.color)
                };
                let both = [
                    seen(EdgeFade::default()),
                    seen(EdgeFade::default().color(Color::from_rgb8(200, 0, 0))),
                ];
                SEEN.with(|cell| cell.borrow_mut().extend(both));
                text("body")
            }
        }
        let mut counter = 0u64;
        let view = frust::component(Themed);
        let _ = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let surface = Theme::neutral().scheme().surface;
        let seen = SEEN.with(|cell| cell.borrow().clone());
        assert_eq!(seen, vec![Some(surface), Some(red)]);
    }

    #[test]
    fn changing_only_the_fade_alpha_requests_paint() {
        let _guard = chrome::slot_test_guard();
        let first = scaffold();
        let (mut element, _) = build(&first);
        let second = scaffold().edge_fade(EdgeFade::default().bar_alpha(0.3));
        let mut counter = 0u64;
        let (flags, _) = second.sync::<()>(&mut element, &mut BuildCtx::new(&mut counter));
        assert!(flags.contains(ChangeFlags::PAINT));
        teardown(&second, &mut element);
    }
}
