//! `native_scaffold(body)`: the app-facing chrome builder (decision C1).
//!
//! The tab bar, title, leading item and actions are the *host's*, not a
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
//! # Insets
//!
//! On a target with a chrome arm the host already reports the bars through the
//! window insets (iOS safe area) or shrinks the surface (Android), so the body
//! uses the facade's `safe_area` as usual. The scaffold adds no padding of its
//! own: do not double-pad. The fallback bars (`crate::chrome::fallback`) take
//! the top and bottom insets themselves and lay the body out between them
//! with those edges consumed, so the same `safe_area` body pads only the sides.

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
    fallback_tab_row, fallback_top_bar, teardown_tab_row, teardown_top_bar,
};
use crate::chrome::{
    self, ActionId, ActionItem, BARS_FLOAT_OVER_BODY, ChromeEvent, ChromeGeneration, ChromeSpec,
    Leading, MenuItemId, NATIVE_CHROME_ARM, PublishOutcome, ScrollState, TopBarSpec,
};

/// How far past a bar's inset line the edge fade reaches into the body, in
/// logical pixels.
const EDGE_FADE_TAIL: f64 = 24.0;
/// The fade's alpha at the screen edge and where it crosses the inset line.
const EDGE_FADE_EDGE_ALPHA: f32 = 0.92;
const EDGE_FADE_INSET_ALPHA: f32 = 0.6;

/// The refusal banner's label.
const REFUSAL_LABEL: &str = "one native_scaffold per app";
/// Its description.
const REFUSAL_DESCRIPTION: &str = "this scaffold publishes no chrome";

/// App chrome around `body`: tabs, title, subtitle, leading item and actions
/// (an action with a menu is a pull-down). See the [module docs](self) for the
/// controlled contract and the inset rule.
///
/// ```
/// use frust::{Component, Get, RwSignal, Set, View, text};
/// use frust_native_widgets::{ActionItem, Leading, TabIcon, TabId, TabItem, native_scaffold};
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
        scroll: None,
        fade: default_edge_fade(),
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
    scroll: Option<ScrollController>,
    fade: Option<Color>,
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

    /// Paint the edge fade under the floating bars in `color` instead of the
    /// theme surface. Only drawn where the bars float over the body (iOS).
    pub fn edge_fade(mut self, color: Color) -> Self {
        self.fade = Some(color);
        self
    }

    /// Draw no edge fade under the floating bars.
    pub fn no_edge_fade(mut self) -> Self {
        self.fade = None;
        self
    }

    /// The spec this builder publishes, its brightness from the ambient theme.
    fn spec(&self) -> ChromeSpec {
        ChromeSpec {
            tabs: self.tabs.clone(),
            selected: self.selected.clone(),
            top: self.top.clone(),
            scroll: self.scroll.as_ref().and_then(controller_state),
            dark: ambient_dark(),
        }
    }
}

/// The app theme's brightness through the controls' fold ([`theme::is_dark`]);
/// light when no [`Theme`] is threaded, as the controls degrade.
fn ambient_dark() -> bool {
    use_context::<Theme>().as_ref().is_some_and(theme::is_dark)
}

/// The default fade colour: the threaded theme's surface, else opaque black
/// when the ambient brightness is dark and white otherwise.
fn default_edge_fade() -> Option<Color> {
    Some(match use_context::<Theme>() {
        Some(theme) => theme.scheme().surface,
        None if ambient_dark() => Color::BLACK,
        None => Color::WHITE,
    })
}

/// One fade band: its origin and size within the scaffold, and the fraction
/// of the band (from the screen edge inward) that ends at the inset line.
type EdgeBand = (Point, Size, f64);

/// The top and bottom fade bands for bar insets `top` and `bottom`; an edge
/// without an inset gets none.
fn edge_fade_bands(size: Size, top: f64, bottom: f64) -> [Option<EdgeBand>; 2] {
    let band = |inset: f64, at_bottom: bool| {
        (inset > 0.0).then(|| {
            let height = inset + EDGE_FADE_TAIL;
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
fn edge_fade_color(floats: bool, native: bool, fade: Option<Color>) -> Option<Color> {
    fade.filter(|_| floats && native)
}

/// A band's gradient, running from its screen edge (`top`: the upper one)
/// inward through the inset line to transparent.
fn edge_fade_brush(origin: Point, size: Size, solid_until: f64, top: bool, color: Color) -> Brush {
    let (from, to) = if top {
        (origin.y, origin.y + size.height)
    } else {
        (origin.y + size.height, origin.y)
    };
    let stops = [
        (0.0f32, color.multiply_alpha(EDGE_FADE_EDGE_ALPHA)),
        (
            solid_until as f32,
            color.multiply_alpha(EDGE_FADE_INSET_ALPHA),
        ),
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

/// The plugin's refusal banner, unsized, rebuilt in place.
fn refusal<Outer: 'static>() -> AnyView<Outer> {
    banner_placeholder(
        None,
        REFUSAL_LABEL.to_string(),
        REFUSAL_DESCRIPTION.to_string(),
    )
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
    queued: bool,
    schedule: ScheduleFlush,
}

impl ScrollPublisher {
    fn new(generation: Option<ChromeGeneration>, spec: ChromeSpec) -> Self {
        Self {
            generation,
            spec,
            queued: false,
            schedule: spawn_flush,
        }
    }

    /// Adopt what the view just published.
    fn adopt(&mut self, generation: Option<ChromeGeneration>, spec: ChromeSpec) {
        self.generation = generation;
        self.spec = spec;
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
    /// the slot forgets its generation, so it never publishes again.
    fn flush(&mut self) -> Option<PublishOutcome> {
        self.queued = false;
        let generation = self.generation?;
        let outcome = chrome::publish(Some(generation), self.spec.clone());
        if matches!(outcome, PublishOutcome::Stale | PublishOutcome::Refused(_)) {
            self.generation = None;
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
    top: Option<ChildPod>,
    bottom: Option<ChildPod>,
    generation: Option<ChromeGeneration>,
    scroll: Option<ScrollLink>,
    fade: Option<Color>,
    /// Whether a native arm holds the bars (not the fallback, not refused).
    native: bool,
}

/// Whether `outcome` leaves the bars to frust: always without an arm, and on
/// a refused install with one. A refused publisher holds no chrome at all.
fn draws_fallback(outcome: &PublishOutcome) -> bool {
    match outcome {
        PublishOutcome::Refused(_) | PublishOutcome::Stale => false,
        PublishOutcome::Fallback(..) => true,
        _ => !NATIVE_CHROME_ARM,
    }
}

type Retire = fn(&mut ChildPod, &mut BuildCtx<'_>);

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
        let spec = self.spec();
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
        let mut flags = ChangeFlags::NONE;
        match (generation.is_some(), element.banner.is_some()) {
            (false, false) => {
                element.banner = Some(build_child(&refusal::<Outer>(), ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (false, true) => {
                let banner = element.banner.as_mut().expect("checked above");
                flags |= rebuild_child(&refusal::<Outer>(), &refusal::<Outer>(), banner, ctx);
            }
            (true, true) => {
                if let Some(mut banner) = element.banner.take() {
                    teardown_child(&refusal::<Outer>(), &mut banner, ctx);
                }
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (true, false) => {}
        }
        let drawn = generation.filter(|_| draws_fallback(&outcome));
        let top = drawn
            .filter(|_| spec.top.is_some())
            .map(|generation| fallback_top_bar::<Outer>(&spec, generation));
        let tabs = drawn
            .filter(|_| !spec.tabs.is_empty())
            .map(|generation| fallback_tab_row::<Outer>(&spec, generation));
        flags |= sync_bar(&mut element.top, top, teardown_top_bar::<Outer>, ctx);
        flags |= sync_bar(&mut element.bottom, tabs, teardown_tab_row::<Outer>, ctx);
        let native = generation.is_some() && !draws_fallback(&outcome);
        if element.native != native || element.fade != self.fade {
            flags |= ChangeFlags::PAINT;
        }
        element.native = native;
        element.fade = self.fade;
        self.sync_scroll(element, generation, spec);
        (flags, outcome)
    }

    /// (Re)subscribe to the scroll controller, if any.
    fn sync_scroll(
        &self,
        element: &mut ScaffoldWidget,
        generation: Option<ChromeGeneration>,
        spec: ChromeSpec,
    ) {
        let Some(controller) = &self.scroll else {
            // A flush queued before `.scroll(..)` went away must publish nothing.
            if let Some(link) = element.scroll.take() {
                link.invalidate();
            }
            return;
        };
        let publisher = match element.scroll.take() {
            Some(link) => link.publisher,
            None => Rc::new(RefCell::new(ScrollPublisher::new(generation, spec.clone()))),
        };
        publisher.borrow_mut().adopt(generation, spec);
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
            top: None,
            bottom: None,
            generation: None,
            scroll: None,
            fade: None,
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
        if let Some(mut banner) = element.banner.take() {
            teardown_child(&refusal::<Outer>(), &mut banner, ctx);
        }
        if let Some(mut top) = element.top.take() {
            teardown_top_bar::<Outer>(&mut top, ctx);
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
        self.top.is_some() || self.bottom.is_some()
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
        let Some(color) = edge_fade_color(floats, self.native, self.fade) else {
            return;
        };
        let bands = edge_fade_bands(size, top, bottom);
        for (index, band) in bands.into_iter().enumerate() {
            if let Some((offset, band_size, solid_until)) = band {
                let at = Point::new(origin.x + offset.x, origin.y + offset.y);
                let brush = edge_fade_brush(at, band_size, solid_until, index == 0, color);
                scene.fill_rect_brush(at, band_size, &brush);
            }
        }
    }

    /// The insets the body sees: the edges the fallback bars took, consumed.
    fn body_insets(&self, insets: WindowInsets) -> WindowInsets {
        insets.consuming(false, self.top.is_some(), false, self.bottom.is_some())
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
        if !self.has_bars() {
            let body_bc = BoxConstraints::new(
                Size::new(bc.min().width, (bc.min().height - banner_height).max(0.0)),
                Size::new(bc.max().width, (bc.max().height - banner_height).max(0.0)),
            );
            let body = self.body.layout_child(ctx, &body_bc);
            self.body.set_origin(Point::new(0.0, banner_height));
            return bc.constrain(Size::new(body.width, body.height + banner_height));
        }
        // Column: banner, top bar, body (flexible), tab row — bars full width.
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
        let bottom_height = bar_height(&mut self.bottom);
        let used = banner_height + top_height + bottom_height;
        let body_bc = if bc.max().height.is_finite() {
            let height = (bc.max().height - used).max(0.0);
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
        let insets = self.body_insets(ctx.window_insets());
        let body = &mut self.body;
        let body_size = ctx.with_window_insets(insets, |ctx| body.layout_child(ctx, &body_bc));
        let body_y = banner_height + top_height;
        self.body.set_origin(Point::new(0.0, body_y));
        if let Some(top) = &mut self.top {
            top.set_origin(Point::new(0.0, banner_height));
        }
        if let Some(bottom) = &mut self.bottom {
            bottom.set_origin(Point::new(0.0, body_y + body_size.height));
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
        for pod in [&mut self.top, &mut self.bottom, &mut self.banner]
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
            .chain(self.bottom.as_mut())
            .collect();
        route_pods(&mut pods, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in [&self.top, &self.banner].into_iter().flatten() {
            pod.semantics_child(ctx);
        }
        self.body.semantics_child(ctx);
        if let Some(bottom) = &self.bottom {
            bottom.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(body, banner, top, bottom);
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
        let mut publisher = ScrollPublisher::new(generation, view.spec());
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
            let mut publisher = ScrollPublisher::new(Some(withdrawn), view.spec());
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
            !NATIVE_CHROME_ARM
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
        let [top, bottom] = edge_fade_bands(Size::new(375.0, 667.0), 74.0, 83.0);
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
        let [top, bottom] = edge_fade_bands(Size::new(375.0, 667.0), 0.0, 83.0);
        assert!(top.is_none() && bottom.is_some());
        assert_eq!(
            edge_fade_bands(Size::new(375.0, 667.0), 0.0, 0.0),
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
        let on = scaffold().edge_fade(red);
        let mut element = faded(&on, true);
        assert_eq!(element.fade, Some(red));
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
                SEEN.with(|seen| seen.set(Some(scaffold().fade)));
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
        assert_eq!(scaffold().fade, Some(Color::WHITE), "no theme");
    }
}
