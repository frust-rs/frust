//! `native_scaffold(body)`: the app-facing chrome builder (decision C1).
//!
//! The tab bar, title, leading item and actions are the *host's*, not a
//! `platform_view` slot: the scaffold builds a `ChromeSpec` from its builder
//! methods and publishes it to the process-wide slot in `crate::chrome`, where
//! an arm (or, on a target without one, the fallback chrome) owns the real
//! bars. The body renders full-bleed. A second live scaffold publishes
//! nothing and shows a one-line refusal above its body.
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
//! own: do not double-pad.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use frust::authoring::{build_child, rebuild_child, route_event_single, teardown_child};
use frust::{RwSignal, ScrollController, ScrollInfo, ScrollSubscription, Set, text};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

use super::builders::{TabId, TabItem};
use super::signals::{ChromeHandlers, on_chrome};
use crate::chrome::{
    self, ActionId, ActionItem, ChromeEvent, ChromeGeneration, ChromeSpec, Leading, MenuItemId,
    PublishOutcome, ScrollState, TopBarSpec,
};

/// What a refused (second) scaffold shows above its body.
const REFUSAL_MESSAGE: &str = "native_scaffold: one per app — this one publishes no chrome";

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

    /// The spec this builder publishes.
    fn spec(&self) -> ChromeSpec {
        ChromeSpec {
            tabs: self.tabs.clone(),
            selected: self.selected.clone(),
            top: self.top.clone(),
            scroll: self.scroll.as_ref().and_then(controller_state),
        }
    }
}

/// The controller's current position, if a surface is attached.
fn controller_state(controller: &ScrollController) -> Option<ScrollState> {
    controller.is_attached().then(|| ScrollState {
        offset: controller.offset(),
        max_offset: controller.max_offset(),
        viewport: controller.viewport_extent(),
    })
}

/// The refusal line, erased so it can be rebuilt in place.
fn refusal<Outer: 'static>() -> AnyView<Outer> {
    any(text(REFUSAL_MESSAGE))
}

/// Coalesces scroll reports into at most one publish per frame: the listener
/// records the newest value (`observe`) and the frame task publishes it
/// (`flush`).
struct ScrollPublisher {
    generation: Option<ChromeGeneration>,
    spec: ChromeSpec,
    viewport: f64,
    queued: bool,
}

impl ScrollPublisher {
    /// Adopt what the view just published.
    fn adopt(&mut self, generation: Option<ChromeGeneration>, spec: ChromeSpec) {
        self.viewport = spec.scroll.map_or(self.viewport, |scroll| scroll.viewport);
        self.generation = generation;
        self.spec = spec;
    }

    /// Record `info`; `true` when a flush must be scheduled (the value changed
    /// and none is already queued).
    fn observe(&mut self, info: ScrollInfo) -> bool {
        let state = ScrollState {
            offset: info.offset,
            max_offset: info.max_offset,
            viewport: self.viewport,
        };
        if self.spec.scroll == Some(state) {
            return false;
        }
        self.spec.scroll = Some(state);
        !std::mem::replace(&mut self.queued, true)
    }

    /// Publish the newest recorded state.
    fn flush(&mut self) -> Option<PublishOutcome> {
        self.queued = false;
        let generation = self.generation?;
        Some(chrome::publish(Some(generation), self.spec.clone()))
    }
}

/// The live scroll subscription and the publisher its listener feeds.
struct ScrollLink {
    publisher: Rc<RefCell<ScrollPublisher>>,
    _subscription: ScrollSubscription,
}

/// The retained widget of a [`NativeScaffoldView`]: the body, the refusal
/// line when refused, and the chrome publication it owns.
pub struct ScaffoldWidget {
    body: ChildPod,
    banner: Option<ChildPod>,
    generation: Option<ChromeGeneration>,
    scroll: Option<ScrollLink>,
}

impl<V> NativeScaffoldView<V> {
    /// Publish this builder's spec for `element` and bring its banner and
    /// scroll subscription in line with the outcome.
    fn sync<Outer: 'static>(
        &self,
        element: &mut ScaffoldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> (ChangeFlags, PublishOutcome) {
        let spec = self.spec();
        let outcome = chrome::publish(element.generation, spec.clone());
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
            element.scroll = None;
            return;
        };
        let publisher = match element.scroll.take() {
            Some(link) => link.publisher,
            None => Rc::new(RefCell::new(ScrollPublisher {
                generation,
                spec: spec.clone(),
                viewport: 0.0,
                queued: false,
            })),
        };
        publisher.borrow_mut().adopt(generation, spec);
        let listener = Rc::clone(&publisher);
        // `ScrollController` has no identity to compare, so every rebuild
        // resubscribes; the publisher (and its coalescing flag) carries over.
        let subscription = controller.on_change(move |info| {
            if listener.borrow_mut().observe(info) {
                let task = Rc::clone(&listener);
                frust::spawn_local(async move {
                    // The borrow ends before `flush`'s publish result is dropped.
                    let _ = task.borrow_mut().flush();
                });
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
            generation: None,
            scroll: None,
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
        element.scroll = None;
        if let Some(generation) = element.generation.take() {
            chrome::withdraw(generation);
        }
    }
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
        let body_bc = BoxConstraints::new(
            Size::new(bc.min().width, (bc.min().height - banner_height).max(0.0)),
            Size::new(bc.max().width, (bc.max().height - banner_height).max(0.0)),
        );
        let body = self.body.layout_child(ctx, &body_bc);
        self.body.set_origin(Point::new(0.0, banner_height));
        bc.constrain(Size::new(body.width, body.height + banner_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.body.paint_child(ctx, scene);
        if let Some(banner) = &mut self.banner {
            banner.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.body, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.body.semantics_child(ctx);
        if let Some(banner) = &self.banner {
            banner.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(body, banner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{MenuItem, TabIcon};
    use crate::chrome::{decode_event, encode_event};
    use std::sync::Mutex;

    /// The slot is process-wide: every test that publishes serialises here
    /// and withdraws what it installed.
    static SLOT_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SLOT_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

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
        let _guard = lock();
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
        let _guard = lock();
        let view = scaffold();
        let (mut first, _) = build(&view);
        teardown(&view, &mut first);
        let (mut second, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Installed(_)));
        teardown(&view, &mut second);
    }

    #[test]
    fn a_second_live_scaffold_is_refused_and_shows_the_banner() {
        let _guard = lock();
        let view = scaffold();
        let (mut first, _) = build(&view);
        let (mut second, outcome) = build(&view);
        assert!(matches!(outcome, PublishOutcome::Refused(_)));
        assert!(second.banner.is_some() && second.generation.is_none());
        assert!(first.banner.is_none());
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
        let _guard = lock();
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
        let _guard = lock();
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
        let _guard = lock();
        let view = scaffold();
        let (mut element, _) = build(&view);
        let generation = element.generation;
        let mut publisher = ScrollPublisher {
            generation,
            spec: view.spec(),
            viewport: 600.0,
            queued: false,
        };
        let at = |offset| ScrollInfo {
            offset,
            max_offset: 400.0,
            overscroll: 0.0,
        };
        assert!(
            publisher.observe(at(10.0)),
            "first change schedules a flush"
        );
        assert!(!publisher.observe(at(20.0)), "a flush is already queued");
        assert!(matches!(
            publisher.flush(),
            Some(PublishOutcome::Applied(_))
        ));
        assert!(
            !publisher.observe(at(20.0)),
            "an unchanged value schedules nothing"
        );
        assert!(publisher.observe(at(30.0)));
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

    #[test]
    fn layout_stacks_the_banner_above_the_body() {
        let _guard = lock();
        let view = scaffold();
        let (mut first, _) = build(&view);
        let (mut second, _) = build(&view);
        let bc = BoxConstraints::loose(Size::new(300.0, 500.0));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn std::any::Any);
        let plain = first.layout(&mut lctx, &bc);
        let stacked = second.layout(&mut lctx, &bc);
        assert!(stacked.height > plain.height, "the banner adds height");
        teardown(&view, &mut first);
        teardown(&view, &mut second);
    }
}
