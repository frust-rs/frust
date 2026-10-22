//! Native Widgets Demo — the showcase and device-gate vehicle for
//! `plugins/native-widgets`, shaped like the GitHub app: native chrome (the
//! [`native_scaffold`] top bar, tab bar and toolbar) and native main controls
//! around a frust-drawn body.
//!
//! This module is the **shell**: [`NativeWidgetsDemoState`], the root
//! [`NativeWidgetsDemoApp`] whose `build` is one `native_scaffold` tree over a
//! reach-gated [`indexed_stack`] of four tab components ([`Tab`]), each a
//! [`navigator`] observed by its own [`RouteObserver`], and the
//! [`frust::app!`] entry. The tab bodies live in [`pages`].
//!
//! **Chrome.** At a navigator's root the bar shows the selected tab's title
//! (the avatar leads on Home); above it, the pushed page's [`DetailRoute`]
//! title and subtitle and a back item. Chrome handlers get no `&mut State` and
//! must be `Send + Sync`, so they write signals only: a pop they ask for is a
//! queued [`ChromeRequest`] the shell's next build applies.
//!
//! **Deep links.** `native-widgets-demo://section/<label>` opens the tab an old
//! section label now lives on and asks it to scroll to that section
//! ([`AnchorRequest`]); `native-widgets-demo://tab/<slug>` opens a tab.
//!
//! **Mode B.** Under the iOS translucent surface every unpainted pixel shows
//! through, so the body's bottom layer is a surface-coloured [`AppBackground`].

pub mod pages;

use std::cell::Cell;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};
use frust::{
    AnyView, BackFallback, Brightness, Color, Component, DeepLink, Get, GetUntracked, Location,
    NavigatorController, NavigatorView, PushOptions, RouteObserver, RouteParams, RwSignal,
    ScrollController, Set, SizedBox, SizedBoxView, Theme, TransitionSpec, Update, View,
    back_fallback, component, deep_links, indexed_stack, navigator, set_app_theme, stack,
    use_context,
};
use frust_glyph::{baseline, toast_host};
use frust_native_widgets::{
    ActionId, ActionItem, ChromeEvent, Leading, MenuItem, MenuItemId, NativeScaffoldView, TabIcon,
    TabId, TabItem, ToolbarSpec, native_scaffold,
};

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

/// The four bottom tabs, in bar order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tab {
    /// The GitHub-shaped home: work rows pushing detail pages.
    #[default]
    Home,
    /// The side-by-side control catalog.
    Controls,
    /// Alerts, sheets and the chrome's own menus.
    Dialogs,
    /// Slot accounting, stress, sandwich and the macOS reference.
    Lab,
}

impl Tab {
    /// How many tabs there are.
    pub const COUNT: usize = 4;

    /// Every tab, in bar order (`ALL[tab.index()] == tab`).
    pub const ALL: [Tab; Tab::COUNT] = [Tab::Home, Tab::Controls, Tab::Dialogs, Tab::Lab];

    /// This tab's position in [`Tab::ALL`], the `indexed_stack` index.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The tab's id and `tab/<slug>` deep-link slug.
    pub const fn slug(self) -> &'static str {
        match self {
            Tab::Home => "home",
            Tab::Controls => "controls",
            Tab::Dialogs => "dialogs",
            Tab::Lab => "lab",
        }
    }

    /// The tab's label and its navigator root's bar title.
    pub const fn title(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Controls => "Controls",
            Tab::Dialogs => "Dialogs",
            Tab::Lab => "Lab",
        }
    }

    /// The tab whose slug is `slug`, case-insensitively.
    pub fn from_slug(slug: &str) -> Option<Tab> {
        Tab::ALL
            .into_iter()
            .find(|tab| tab.slug().eq_ignore_ascii_case(slug))
    }

    /// The SF Symbol the iOS tab bar shows.
    const fn symbol(self) -> &'static str {
        match self {
            Tab::Home => "house",
            Tab::Controls => "slider.horizontal.3",
            Tab::Dialogs => "bubble.left.and.bubble.right",
            Tab::Lab => "flask",
        }
    }

    /// The `assets/icons` PNG every other platform shows.
    fn png(self) -> Arc<[u8]> {
        static PNGS: OnceLock<[Arc<[u8]>; Tab::COUNT]> = OnceLock::new();
        let pngs = PNGS.get_or_init(|| {
            [
                Arc::from(&include_bytes!("../assets/icons/home.png")[..]),
                Arc::from(&include_bytes!("../assets/icons/controls.png")[..]),
                Arc::from(&include_bytes!("../assets/icons/dialogs.png")[..]),
                Arc::from(&include_bytes!("../assets/icons/lab.png")[..]),
            ]
        });
        Arc::clone(&pngs[self.index()])
    }
}

/// The Home avatar (`assets/icons/avatar.png`), published once so every
/// build hands the chrome the same bytes.
fn avatar_png() -> Arc<[u8]> {
    static AVATAR: OnceLock<Arc<[u8]>> = OnceLock::new();
    Arc::clone(AVATAR.get_or_init(|| Arc::from(&include_bytes!("../assets/icons/avatar.png")[..])))
}

/// An icon: the SF Symbol `symbol` on iOS, `png` everywhere else. macOS and
/// the other desktop targets draw the fallback chrome, which has no SF Symbol
/// arm, so they take the bitmap asset.
fn chrome_icon(symbol: &str, png: Arc<[u8]>) -> TabIcon {
    if cfg!(target_os = "ios") {
        TabIcon::AppleSymbol(symbol.to_string())
    } else {
        TabIcon::Bytes(png)
    }
}

/// Publishes the `assets/icons` PNG `bytes` once in `cell`, so every build
/// hands the chrome the same bytes.
fn published_png(cell: &'static OnceLock<Arc<[u8]>>, bytes: &'static [u8]) -> Arc<[u8]> {
    Arc::clone(cell.get_or_init(|| Arc::from(bytes)))
}

/// The New action's `plus.png`.
fn plus_png() -> Arc<[u8]> {
    static PLUS_PNG: OnceLock<Arc<[u8]>> = OnceLock::new();
    published_png(&PLUS_PNG, include_bytes!("../assets/icons/plus.png"))
}

/// The Search action's `search.png`.
fn search_png() -> Arc<[u8]> {
    static SEARCH_PNG: OnceLock<Arc<[u8]>> = OnceLock::new();
    published_png(&SEARCH_PNG, include_bytes!("../assets/icons/search.png"))
}

/// The More actions' `more.png`.
fn more_png() -> Arc<[u8]> {
    static MORE_PNG: OnceLock<Arc<[u8]>> = OnceLock::new();
    published_png(&MORE_PNG, include_bytes!("../assets/icons/more.png"))
}

// ---------------------------------------------------------------------------
// Old section labels, destinations and anchors
// ---------------------------------------------------------------------------

/// The old per-family section labels, kept as `section/<label>` deep-link
/// slugs and as the anchors their tabs scroll to. [`pages::current`] still
/// maps an index to the section's own page.
pub const SECTION_LABELS: [&str; 8] = [
    "Controls",
    "macOS",
    "New",
    "Alerts",
    "TabBar",
    "Sheet",
    "Composite",
    "Stress",
];

/// The Sandwich section's label and slug: routed like a section, after the
/// eight [`SECTION_LABELS`], without joining the per-family tables.
pub const SANDWICH_LABEL: &str = "Sandwich";

/// The Sandwich section's index: one past the last of [`SECTION_LABELS`].
pub const SANDWICH_SECTION: usize = SECTION_LABELS.len();

/// The tab each section lives on, index-aligned with [`section_labels`].
const SECTION_TABS: [Tab; SECTION_LABELS.len() + 1] = [
    Tab::Controls,
    Tab::Lab,
    Tab::Controls,
    Tab::Dialogs,
    Tab::Lab,
    Tab::Dialogs,
    Tab::Controls,
    Tab::Lab,
    Tab::Lab,
];

/// Every section label in order: the [`SECTION_LABELS`], then [`SANDWICH_LABEL`].
fn section_labels() -> impl Iterator<Item = &'static str> {
    SECTION_LABELS
        .iter()
        .copied()
        .chain(std::iter::once(SANDWICH_LABEL))
}

/// A section label's index (case-insensitive), [`SANDWICH_SECTION`] for
/// [`SANDWICH_LABEL`], `None` for an unknown label.
pub fn section_index_for(label: &str) -> Option<usize> {
    section_labels().position(|s| s.eq_ignore_ascii_case(label))
}

/// Where a deep link or a launch value lands: a tab, and the section anchor
/// (the canonical section label) its body scrolls to, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destination {
    /// The tab to select.
    pub tab: Tab,
    /// The section label to scroll to; `None` leaves the tab where it is.
    pub anchor: Option<&'static str>,
}

/// The destination of a `section/<label>` link: the section's tab and anchor.
pub fn section_destination(label: &str) -> Option<Destination> {
    let index = section_index_for(label)?;
    Some(Destination {
        tab: SECTION_TABS[index],
        anchor: section_labels().nth(index),
    })
}

/// The destination of a `tab/<slug>` link: the tab, no anchor.
pub fn tab_destination(slug: &str) -> Option<Destination> {
    Tab::from_slug(slug).map(|tab| Destination { tab, anchor: None })
}

/// A request for a tab body to scroll to the section `anchor`, stamped with a
/// `sequence` the shell bumps per request, so a body applies each request
/// once by remembering the last sequence it handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRequest {
    /// The section label to scroll to.
    pub anchor: &'static str,
    /// Increases with every request.
    pub sequence: u64,
}

/// The debug-only environment variable naming the page to open at launch: a
/// section label or a tab slug, case-insensitive (the desktop shell has no
/// launch-time deep link). Unset, blank or unknown: Home. Read once in `init`,
/// only in debug builds.
pub const SECTION_ENV_VAR: &str = "FRUST_DEMO_SECTION";

/// The destination a [`SECTION_ENV_VAR`] value selects (surrounding whitespace
/// ignored): a section label first, then a tab slug.
pub fn destination_from_env_value(value: Option<&str>) -> Option<Destination> {
    let label = value?.trim();
    if label.is_empty() {
        return None;
    }
    section_destination(label).or_else(|| tab_destination(label))
}

// ---------------------------------------------------------------------------
// Deep-link routing — pure parse/plan functions, applied by the shell's
// `deep_link_router`
// ---------------------------------------------------------------------------

/// The scheme every routed deep link starts with.
const DEEP_LINK_SCHEME: &str = "native-widgets-demo://";

/// Which route family a deep link names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkKind {
    /// `section/<label>`: an old section label.
    Section,
    /// `tab/<slug>`: a tab slug.
    Tab,
}

/// Split a `native-widgets-demo://<section|tab>/<label>[/...]` URL into its
/// kind and label, taking the label up to the first `/`, `?` or `#`. `None`
/// for another scheme or host, or an empty label. No percent-decoding: every
/// label and slug is a plain ASCII word.
fn parse_deep_link(url: &str) -> Option<(LinkKind, &str)> {
    let (host, path) = url.strip_prefix(DEEP_LINK_SCHEME)?.split_once('/')?;
    let kind = match host {
        "section" => LinkKind::Section,
        "tab" => LinkKind::Tab,
        _ => return None,
    };
    let label = path.split(['/', '?', '#']).next().unwrap_or("");
    (!label.is_empty()).then_some((kind, label))
}

/// The longest unrecognized deep-link label [`sanitize_deep_link_label`]
/// ever echoes verbatim into UI text.
const MAX_ECHOED_LABEL_LEN: usize = 32;

/// `label` if it is safe to echo in a toast: ASCII alphanumerics and `-`
/// only, at most [`MAX_ECHOED_LABEL_LEN`] characters. An unknown label is
/// whatever any process on the device handed this app's scheme, so anything
/// else is dropped (the router shows a fixed message and logs it escaped).
fn sanitize_deep_link_label(label: &str) -> Option<&str> {
    let ok = !label.is_empty()
        && label.len() <= MAX_ECHOED_LABEL_LEN
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ok.then_some(label)
}

/// How many **characters** of an unknown label the warning log echoes,
/// counted on the raw label before `{:?}` escaping: slicing the escaped form
/// at a byte offset can land inside a multi-byte character and panic.
const MAX_LOGGED_LABEL_CHARS: usize = 128;

/// The log form of an unknown label: its `{:?}`-escaped prefix of at most
/// [`MAX_LOGGED_LABEL_CHARS`] characters (`...` marks a cut) plus the raw
/// byte length. Never panics on any `&str`.
fn logged_deep_link_label(label: &str) -> String {
    let byte_len = label.len();
    let prefix: String = label.chars().take(MAX_LOGGED_LABEL_CHARS).collect();
    if prefix.len() < byte_len {
        format!("{prefix:?}... ({byte_len} bytes)")
    } else {
        format!("{label:?} ({byte_len} bytes)")
    }
}

/// The minimum interval between two unknown-label toasts; a suppressed one is
/// still logged.
const UNKNOWN_LABEL_TOAST_INTERVAL: Duration = Duration::from_secs(2);

/// Whether an unknown-label toast at `now` is suppressed, given when the last
/// one was shown.
fn should_rate_limit_unknown_toast(last_toast_at: Option<Instant>, now: Instant) -> bool {
    last_toast_at.is_some_and(|last| now.duration_since(last) < UNKNOWN_LABEL_TOAST_INTERVAL)
}

/// Append `message` to the toast log — append-only, never collapsing:
/// [`toast_host`] shows each entry once by index, so a repeat is a new
/// request and draining from the front would shift its cursor. Growth is
/// bounded at the producers ([`should_rate_limit_unknown_toast`] gates the
/// only toast an outside process can trigger).
fn append_toast(queue: &mut Vec<String>, message: String) {
    queue.push(message);
}

/// Push `message` onto `toasts` via [`append_toast`] — every toast producer in
/// this app goes through here.
pub(crate) fn push_toast(toasts: RwSignal<Vec<String>>, message: String) {
    toasts.update(|queue| append_toast(queue, message));
}

/// What a delivered [`DeepLink`] resolves to, applied by the shell's router.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeepLinkAction {
    /// Show this destination.
    Go(Destination),
    /// A routed link whose label matched nothing — the raw label, for the
    /// toast and the log.
    UnknownLabel(String),
}

/// Decide what a delivered `link` should do, given the sequence of the last
/// delivery already applied. Dedupes on [`DeepLink::sequence`], so two
/// deliveries of an identical URL are two applications while a rebuild
/// re-observing one delivery is a no-op. `None` for an applied delivery or a
/// URL this app does not route.
fn plan_deep_link(link: &DeepLink, last_applied: Option<u64>) -> Option<DeepLinkAction> {
    if last_applied == Some(link.sequence) {
        return None;
    }
    let (kind, label) = parse_deep_link(&link.url)?;
    let destination = match kind {
        LinkKind::Section => section_destination(label),
        LinkKind::Tab => tab_destination(label),
    };
    Some(destination.map_or_else(
        || DeepLinkAction::UnknownLabel(label.to_string()),
        DeepLinkAction::Go,
    ))
}

// ---------------------------------------------------------------------------
// AppBackground — the Mode B root background layer
// ---------------------------------------------------------------------------

/// A full-bleed background fill, authored through `frust::authoring`
/// (`examples/playground`'s `AppBackground` is the same pattern): [`stack`]
/// hands every child a loose constraint, so declaring a larger-than-any-viewport
/// size and letting [`BoxConstraints::constrain`] clamp it fills the stack.
struct AppBackground(Color);

/// The retained widget for an [`AppBackground`].
struct AppBackgroundWidget(Color);

/// Larger than any real viewport (logical px).
const BACKGROUND_INTRINSIC: f64 = 1.0e7;

impl<State: 'static> View<State> for AppBackground {
    type Element = AppBackgroundWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AppBackgroundWidget {
        AppBackgroundWidget(self.0)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut AppBackgroundWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.0 = self.0;
        ChangeFlags::PAINT
    }
}

impl Widget for AppBackgroundWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(BACKGROUND_INTRINSIC, BACKGROUND_INTRINSIC))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), self.0);
    }
}

// ---------------------------------------------------------------------------
// Pushed-page routes and chrome requests
// ---------------------------------------------------------------------------

/// The route path a [`DetailRoute`] page carries.
const DETAIL_PATH: &str = "/detail";

/// The bar title and subtitle of a pushed detail page. The page carries it as
/// its navigator route ([`DetailRoute::push`]); the shell reads it back from
/// the tab's [`RouteObserver`], so a page never sets chrome itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailRoute {
    /// The bar title.
    pub title: String,
    /// The bar subtitle; empty shows none.
    pub subtitle: String,
}

impl DetailRoute {
    /// A route titled `title` over `subtitle`.
    pub fn new(title: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: subtitle.into(),
        }
    }

    /// The navigator route this page is pushed with.
    pub fn location(&self) -> Location {
        let mut query = RouteParams::new();
        query.insert("title".to_string(), self.title.clone());
        query.insert("subtitle".to_string(), self.subtitle.clone());
        Location {
            path: DETAIL_PATH.to_string(),
            segments: vec![DETAIL_PATH.trim_start_matches('/').to_string()],
            query,
        }
    }

    /// The route a [`Self::location`] encodes; `None` for any other location.
    pub fn from_location(location: &Location) -> Option<Self> {
        if location.path != DETAIL_PATH {
            return None;
        }
        Some(Self {
            title: location.query.get("title")?.clone(),
            subtitle: location.query.get("subtitle").cloned().unwrap_or_default(),
        })
    }

    /// Push `page` onto `nav` as an opaque page carrying this route.
    pub fn push<V: View<NativeWidgetsDemoState>>(
        &self,
        nav: &NavigatorController<NativeWidgetsDemoState>,
        page: impl Fn() -> V + 'static,
    ) {
        nav.push_with_options(page, PushOptions::opaque().route(self.location()));
    }
}

/// A navigator change a chrome handler asked for, applied by the shell's next
/// build (a handler may not hold a `NavigatorController`, which is `!Send`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeRequest {
    /// The bar's back item: pop this tab's navigator.
    Back(Tab),
    /// The showing tab was tapped again: pop to its root, or at the root
    /// scroll its body to the top.
    PopToRoot(Tab),
}

// ---------------------------------------------------------------------------
// State + theme
// ---------------------------------------------------------------------------

/// The app's whole reactive surface. Every tab component starts from a clone:
/// the signals, controllers and observers are shared handles, so a clone
/// shares them; the `Cell`s are the shell's own bookkeeping.
#[derive(Clone)]
pub struct NativeWidgetsDemoState {
    /// Light/Dark, seeded from the theme the shell applied; the one theme
    /// signal every native control and frust-drawn peer follows.
    pub brightness: RwSignal<Brightness>,
    /// The selected tab — written synchronously by the tab bar (iOS selects
    /// optimistically and waits for the app to agree), deep links and back.
    pub tab: RwSignal<Tab>,
    /// The append-only toast log [`toast_host`] shows; push via [`push_toast`].
    pub toasts: RwSignal<Vec<String>>,
    /// One navigator per tab, by [`Tab::index`].
    pub navs: [NavigatorController<NativeWidgetsDemoState>; Tab::COUNT],
    /// One route observer per tab, by [`Tab::index`]; the chrome reads the shown one.
    pub routes: [RouteObserver; Tab::COUNT],
    /// One scroll controller per tab root body, by [`Tab::index`]: every tab
    /// stays laid out, so they cannot share one. The chrome follows the shown one.
    pub scrolls: [ScrollController; Tab::COUNT],
    /// A pushed detail page's body scroll, which the chrome follows meanwhile.
    pub detail_scroll: ScrollController,
    /// The section a tab body should scroll to (see [`AnchorRequest`]).
    pub pending_anchor: RwSignal<Option<AnchorRequest>>,
    /// Whether the detail page's composer is open (the toolbar's Comment toggles it).
    pub composer_open: RwSignal<bool>,
    /// The last chrome event of any kind (`events_into`), for readouts.
    pub chrome_event: RwSignal<Option<ChromeEvent>>,
    /// Presentation outcomes not yet seen: the Dialogs tab's badge while
    /// non-zero; selecting Dialogs clears it.
    pub unseen_outcomes: RwSignal<u32>,
    /// Navigator changes chrome handlers asked for, append-only.
    pub chrome_requests: RwSignal<Vec<ChromeRequest>>,
    /// Back at a tab's root returns to Home; at Home's root it exits.
    pub home_fallback: BackFallback,
    /// How many of [`Self::chrome_requests`] the shell has applied.
    pub applied_requests: Cell<usize>,
    /// The last [`AnchorRequest::sequence`] the shell issued.
    pub anchor_sequence: Cell<u64>,
    /// The [`DeepLink::sequence`] of the last delivery the shell applied.
    pub last_applied_sequence: Cell<Option<u64>>,
    /// When the last unknown-label toast was shown (rate limiting).
    pub last_unknown_toast_at: Cell<Option<Instant>>,
}

impl NativeWidgetsDemoState {
    /// The initial state: Home, the brightness of the theme the shell applied
    /// (Glyph's default before one exists), fresh per-tab handles.
    pub fn new() -> Self {
        let tab = RwSignal::new(Tab::Home);
        let navs: [NavigatorController<NativeWidgetsDemoState>; Tab::COUNT] =
            std::array::from_fn(|_| NavigatorController::new());
        let probe = navs.clone();
        let home_fallback = BackFallback::new(
            move || {
                let shown = tab.get_untracked();
                shown != Tab::Home && probe[shown.index()].depth() <= 1
            },
            move || tab.set(Tab::Home),
        );
        let applied =
            use_context::<Theme>().map_or_else(|| baseline().brightness, |t| t.brightness);
        NativeWidgetsDemoState {
            brightness: RwSignal::new(applied),
            tab,
            toasts: RwSignal::new(Vec::new()),
            navs,
            routes: std::array::from_fn(|_| RouteObserver::new()),
            scrolls: std::array::from_fn(|_| ScrollController::new()),
            detail_scroll: ScrollController::new(),
            pending_anchor: RwSignal::new(None),
            composer_open: RwSignal::new(false),
            chrome_event: RwSignal::new(None),
            unseen_outcomes: RwSignal::new(0),
            chrome_requests: RwSignal::new(Vec::new()),
            home_fallback,
            applied_requests: Cell::new(0),
            anchor_sequence: Cell::new(0),
            last_applied_sequence: Cell::new(None),
            last_unknown_toast_at: Cell::new(None),
        }
    }

    /// `tab`'s navigator.
    pub fn nav(&self, tab: Tab) -> &NavigatorController<NativeWidgetsDemoState> {
        &self.navs[tab.index()]
    }

    /// `tab`'s route observer.
    pub fn route(&self, tab: Tab) -> RouteObserver {
        self.routes[tab.index()]
    }

    /// `tab`'s root-body scroll controller.
    pub fn scroll(&self, tab: Tab) -> &ScrollController {
        &self.scrolls[tab.index()]
    }

    /// Select `destination`'s tab and, with an anchor, ask its body to scroll
    /// there. Event-handler or router writes only.
    pub fn go(&self, destination: Destination) {
        show_tab(self.tab, self.unseen_outcomes, destination.tab);
        if let Some(anchor) = destination.anchor {
            let sequence = self.anchor_sequence.get() + 1;
            self.anchor_sequence.set(sequence);
            self.pending_anchor
                .set(Some(AnchorRequest { anchor, sequence }));
        }
    }
}

impl Default for NativeWidgetsDemoState {
    fn default() -> Self {
        Self::new()
    }
}

/// Select `next`; showing Dialogs marks its outcomes seen.
fn show_tab(tab: RwSignal<Tab>, unseen: RwSignal<u32>, next: Tab) {
    tab.set(next);
    if next == Tab::Dialogs && unseen.get_untracked() != 0 {
        unseen.set(0);
    }
}

/// Force the app-wide theme to [`frust_glyph::baseline`] at `brightness`.
/// Glyph, not a neutral theme, so the native controls' typeface ladder has a
/// design system's bundled faces to attach.
pub fn apply_theme(brightness: Brightness) {
    set_app_theme(Theme::builder(baseline()).brightness(brightness).build());
}

/// The other brightness.
fn flipped(brightness: Brightness) -> Brightness {
    match brightness {
        Brightness::Dark => Brightness::Light,
        Brightness::Light => Brightness::Dark,
    }
}

// ---------------------------------------------------------------------------
// Chrome ids
// ---------------------------------------------------------------------------

/// Home's '+' pull-down.
const ADD: &str = "add";
/// Home's search action.
const SEARCH: &str = "search";
/// Every tab root's overflow pull-down.
const MORE: &str = "more";
/// A pushed detail page's pull-down.
const DETAIL_MORE: &str = "detail-more";
/// The detail toolbar's primary action.
const COMMENT: &str = "comment";
/// The detail toolbar's secondary action.
const INFO: &str = "info";

/// `native_scaffold` takes a subtitle unconditionally; this skips an absent one.
trait SubtitleIf {
    /// Set `subtitle` when it is `Some`.
    fn subtitle_if(self, subtitle: Option<String>) -> Self;
}

impl<V> SubtitleIf for NativeScaffoldView<V> {
    fn subtitle_if(self, subtitle: Option<String>) -> Self {
        match subtitle {
            Some(subtitle) => self.subtitle(subtitle),
            None => self,
        }
    }
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

/// The root [`Component`]; `frust::app!` installs Glyph before any shell
/// construction, `init` wires the state.
#[derive(Default)]
pub struct NativeWidgetsDemoApp;

impl Component for NativeWidgetsDemoApp {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        let state = NativeWidgetsDemoState::new();
        // Debug builds only: a launch-time page for scripted desktop runs.
        #[cfg(debug_assertions)]
        if let Some(destination) =
            destination_from_env_value(std::env::var(SECTION_ENV_VAR).ok().as_deref())
        {
            state.go(destination);
        }
        state
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> impl View<NativeWidgetsDemoState> {
        let tab = state.tab.get();
        let detail = Self::detail(state, tab);
        let composer_open = state.composer_open.get();
        native_scaffold(Self::body(state, tab))
            .tabs(Self::tabs(state), tab.slug())
            .title(Self::title(tab, detail.as_ref()))
            .subtitle_if(Self::subtitle(detail.as_ref()))
            .leading(Self::leading(state, tab))
            .actions(Self::actions(tab, state.route(tab).depth() > 1))
            .toolbar(Self::toolbar(tab, detail.as_ref(), composer_open))
            .scroll(Self::scroll(state, tab))
            .events_into(state.chrome_event)
            .on_tab_select(Self::on_tab_select(state))
            .on_tab_reselect(Self::on_tab_reselect(state))
            .on_action(Self::on_action(state))
            .on_menu(Self::on_menu(state))
            .on_back(Self::on_back(state))
    }
}

impl NativeWidgetsDemoApp {
    /// The body: the background, the tab stack under the back fallback, the
    /// two routers and the toast host.
    fn body(state: &NativeWidgetsDemoState, tab: Tab) -> impl View<NativeWidgetsDemoState> {
        stack()
            .child(AppBackground(Self::surface()))
            .child(back_fallback(
                &state.home_fallback,
                indexed_stack(Some(tab.index()))
                    .child(component(HomeTab::new(state)))
                    .child(component(ControlsTab::new(state)))
                    .child(component(DialogsTab::new(state)))
                    .child(component(LabTab::new(state))),
            ))
            .child(Self::deep_link_router(state))
            .child(Self::chrome_router(state))
            .child(toast_host(state.toasts.get()))
    }

    /// One tab's root: its navigator, observed by its route observer.
    fn tab_root(
        state: &NativeWidgetsDemoState,
        tab: Tab,
        page: fn(&NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState>,
    ) -> NavigatorView<NativeWidgetsDemoState> {
        let handles = state.clone();
        state.route(tab).observe(
            navigator(state.nav(tab), move || page(&handles)).transition(TransitionSpec::glyph()),
        )
    }

    /// The live theme's surface colour.
    fn surface() -> Color {
        use_context::<Theme>()
            .unwrap_or_else(baseline)
            .scheme()
            .surface
    }

    /// The pushed page's route on `tab`, if a page above the root carries one.
    fn detail(state: &NativeWidgetsDemoState, tab: Tab) -> Option<DetailRoute> {
        let route = state.route(tab);
        if route.depth() <= 1 {
            return None;
        }
        route
            .current()
            .as_ref()
            .and_then(DetailRoute::from_location)
    }

    /// The four tabs, Dialogs badged while outcomes are unseen.
    fn tabs(state: &NativeWidgetsDemoState) -> Vec<TabItem> {
        let unseen = state.unseen_outcomes.get();
        Tab::ALL
            .into_iter()
            .map(|tab| {
                let item = TabItem::new(
                    tab.slug(),
                    tab.title(),
                    chrome_icon(tab.symbol(), tab.png()),
                );
                if tab == Tab::Dialogs && unseen > 0 {
                    item.badge(unseen.to_string())
                } else {
                    item
                }
            })
            .collect()
    }

    /// The pushed page's title, else the tab's.
    fn title(tab: Tab, detail: Option<&DetailRoute>) -> String {
        detail.map_or_else(|| tab.title().to_string(), |d| d.title.clone())
    }

    /// The pushed page's subtitle, when it has one.
    fn subtitle(detail: Option<&DetailRoute>) -> Option<String> {
        detail
            .map(|d| d.subtitle.clone())
            .filter(|subtitle| !subtitle.is_empty())
    }

    /// Back above a navigator root, the avatar at Home's root, else nothing.
    fn leading(state: &NativeWidgetsDemoState, tab: Tab) -> Leading {
        if state.route(tab).depth() > 1 {
            Leading::Back
        } else if tab == Tab::Home {
            Leading::Image(avatar_png())
        } else {
            Leading::None
        }
    }

    /// One action: its SF Symbol on iOS, else its `png` icon, with its title
    /// beside either (see [`chrome_icon`]).
    fn action(id: &str, title: &str, symbol: &str, png: Arc<[u8]>) -> ActionItem {
        ActionItem::new(id, title).icon(chrome_icon(symbol, png))
    }

    /// Above a root: the page's Share/Copy link menu. At a root: Home's '+'
    /// menu and search, then every tab's overflow menu.
    fn actions(tab: Tab, pushed: bool) -> Vec<ActionItem> {
        if pushed {
            return vec![
                Self::action(DETAIL_MORE, "More", "ellipsis.circle", more_png()).menu(vec![
                    MenuItem::new("share", "Share"),
                    MenuItem::new("copy-link", "Copy link"),
                ]),
            ];
        }
        let home = (tab == Tab::Home).then(|| {
            [
                Self::action(ADD, "New", "plus", plus_png()).menu(vec![
                    MenuItem::new("new-issue", "New issue"),
                    MenuItem::new("new-repository", "New repository"),
                ]),
                Self::action(SEARCH, "Search", "magnifyingglass", search_png()),
            ]
        });
        home.into_iter()
            .flatten()
            .chain(std::iter::once(
                Self::action(MORE, "More", "ellipsis.circle", more_png()).menu(vec![
                    MenuItem::new("toggle-brightness", "Toggle brightness"),
                    MenuItem::new("reread-slots", "Re-read live slots"),
                    MenuItem::new("about", "About"),
                ]),
            ))
            .collect()
    }

    /// The Comment/Info toolbar while Home shows a detail page with its
    /// composer closed: an open composer sits at the column's foot, where the
    /// toolbar would cover it. Empty (shown as nothing) otherwise.
    fn toolbar(tab: Tab, detail: Option<&DetailRoute>, composer_open: bool) -> ToolbarSpec {
        if tab != Tab::Home || detail.is_none() || composer_open {
            return ToolbarSpec::new();
        }
        ToolbarSpec::new()
            .item(ActionItem::new(INFO, "Info"))
            .primary(
                ActionItem::new(COMMENT, "Comment")
                    .icon(chrome_icon("text.bubble", Tab::Dialogs.png())),
            )
    }

    /// The scroll controller the bars follow: the detail body's while a page
    /// is pushed, else the tab body's.
    fn scroll(state: &NativeWidgetsDemoState, tab: Tab) -> ScrollController {
        if state.route(tab).depth() > 1 {
            state.detail_scroll.clone()
        } else {
            state.scroll(tab).clone()
        }
    }

    /// Select the tapped tab synchronously — never vetoed.
    fn on_tab_select(state: &NativeWidgetsDemoState) -> impl Fn(TabId) + Send + Sync + 'static {
        let (tab, unseen) = (state.tab, state.unseen_outcomes);
        move |id: TabId| {
            if let Some(next) = Tab::from_slug(id.as_str()) {
                show_tab(tab, unseen, next);
            }
        }
    }

    /// The showing tab tapped again: pop it to its root (or scroll to top).
    fn on_tab_reselect(state: &NativeWidgetsDemoState) -> impl Fn(TabId) + Send + Sync + 'static {
        let requests = state.chrome_requests;
        move |id: TabId| {
            if let Some(tab) = Tab::from_slug(id.as_str()) {
                requests.update(|queue| queue.push(ChromeRequest::PopToRoot(tab)));
            }
        }
    }

    /// The bar's back item: pop the showing tab's navigator.
    fn on_back(state: &NativeWidgetsDemoState) -> impl Fn() + Send + Sync + 'static {
        let (tab, requests) = (state.tab, state.chrome_requests);
        move || {
            let shown = tab.get_untracked();
            requests.update(|queue| queue.push(ChromeRequest::Back(shown)));
        }
    }

    /// Plain actions: search, and the detail toolbar's Comment and Info.
    fn on_action(state: &NativeWidgetsDemoState) -> impl Fn(ActionId) + Send + Sync + 'static {
        let (toasts, composer_open) = (state.toasts, state.composer_open);
        move |id: ActionId| match id.as_str() {
            SEARCH => push_toast(toasts, "Search".to_string()),
            COMMENT => composer_open.update(|open| *open = !*open),
            INFO => push_toast(toasts, "Pull request info".to_string()),
            _ => {}
        }
    }

    /// Pull-down choices. The brightness toggle flips the theme the shell
    /// last applied (read at build), so it agrees with what is on screen.
    fn on_menu(
        state: &NativeWidgetsDemoState,
    ) -> impl Fn(ActionId, MenuItemId) + Send + Sync + 'static {
        let (toasts, brightness) = (state.toasts, state.brightness);
        let signalled = brightness.get();
        let shown = use_context::<Theme>().map_or(signalled, |t| t.brightness);
        let refresh = pages::common::live_refresh();
        move |action: ActionId, item: MenuItemId| match (action.as_str(), item.as_str()) {
            (ADD, "new-issue") => push_toast(toasts, "New issue".to_string()),
            (ADD, "new-repository") => push_toast(toasts, "New repository".to_string()),
            (MORE, "toggle-brightness") => {
                let next = flipped(shown);
                brightness.set(next);
                apply_theme(next);
            }
            (MORE, "reread-slots") => refresh.update(|n| *n = n.wrapping_add(1)),
            (MORE, "about") => push_toast(
                toasts,
                "Native chrome and main controls around a frust-drawn body".to_string(),
            ),
            (DETAIL_MORE, "share") => push_toast(toasts, "Share".to_string()),
            (DETAIL_MORE, "copy-link") => push_toast(toasts, "Link copied".to_string()),
            _ => {}
        }
    }

    /// Applies new [`ChromeRequest`]s — the tracked read subscribes the shell
    /// to the queue. Renders nothing.
    fn chrome_router(state: &NativeWidgetsDemoState) -> SizedBoxView<NativeWidgetsDemoState> {
        let requests = state.chrome_requests.get();
        for request in requests.iter().skip(state.applied_requests.get()) {
            let tab = match *request {
                ChromeRequest::Back(tab) => {
                    state.nav(tab).pop();
                    tab
                }
                ChromeRequest::PopToRoot(tab) => {
                    let nav = state.nav(tab);
                    if nav.depth() > 1 {
                        (1..nav.depth()).for_each(|_| nav.pop());
                    } else {
                        state.scroll(tab).jump_to(0.0);
                    }
                    tab
                }
            };
            if tab == Tab::Home && state.composer_open.get_untracked() {
                state.composer_open.set(false);
            }
        }
        state.applied_requests.set(requests.len());
        SizedBox(Some(0.0), Some(0.0))
    }

    /// Applies deep links: the single tracked read of
    /// `deep_links().latest`, planned by [`plan_deep_link`] and stamped into
    /// [`NativeWidgetsDemoState::last_applied_sequence`]. Renders nothing.
    fn deep_link_router(state: &NativeWidgetsDemoState) -> SizedBoxView<NativeWidgetsDemoState> {
        if let Some(link) = deep_links().latest.get() {
            match plan_deep_link(&link, state.last_applied_sequence.get()) {
                Some(DeepLinkAction::Go(destination)) => {
                    state.go(destination);
                    log::info!(
                        "native-widgets-demo deep-link: {} -> {:?}",
                        destination.tab.slug(),
                        destination.anchor
                    );
                }
                Some(DeepLinkAction::UnknownLabel(label)) => {
                    let now = Instant::now();
                    if !should_rate_limit_unknown_toast(state.last_unknown_toast_at.get(), now) {
                        let message = match sanitize_deep_link_label(&label) {
                            Some(safe) => format!("Unknown page '{safe}' in deep link"),
                            None => "Unknown page in deep link".to_string(),
                        };
                        push_toast(state.toasts, message);
                        // Stamped only on an actual toast: a suppressed one
                        // must not extend the quiet window.
                        state.last_unknown_toast_at.set(Some(now));
                    }
                    log::warn!(
                        "native-widgets-demo deep-link: {} unknown",
                        logged_deep_link_label(&label)
                    );
                }
                None => {}
            }
            state.last_applied_sequence.set(Some(link.sequence));
        }
        SizedBox(Some(0.0), Some(0.0))
    }
}

// ---------------------------------------------------------------------------
// Tab components — each one tab's navigator, hosted in the indexed stack
// ---------------------------------------------------------------------------

/// The Home tab: work rows pushing detail pages.
pub struct HomeTab(NativeWidgetsDemoState);

impl HomeTab {
    /// The Home tab over the shell's state.
    pub fn new(shell: &NativeWidgetsDemoState) -> Self {
        Self(shell.clone())
    }
}

impl Component for HomeTab {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        self.0.clone()
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> impl View<NativeWidgetsDemoState> {
        NativeWidgetsDemoApp::tab_root(state, Tab::Home, pages::home::page)
    }
}

/// The Controls tab: the side-by-side catalog.
pub struct ControlsTab(NativeWidgetsDemoState);

impl ControlsTab {
    /// The Controls tab over the shell's state.
    pub fn new(shell: &NativeWidgetsDemoState) -> Self {
        Self(shell.clone())
    }
}

impl Component for ControlsTab {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        self.0.clone()
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> impl View<NativeWidgetsDemoState> {
        NativeWidgetsDemoApp::tab_root(state, Tab::Controls, pages::controls_tab::page)
    }
}

/// The Dialogs tab: native presentations and the chrome's menus.
pub struct DialogsTab(NativeWidgetsDemoState);

impl DialogsTab {
    /// The Dialogs tab over the shell's state.
    pub fn new(shell: &NativeWidgetsDemoState) -> Self {
        Self(shell.clone())
    }
}

impl Component for DialogsTab {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        self.0.clone()
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> impl View<NativeWidgetsDemoState> {
        NativeWidgetsDemoApp::tab_root(state, Tab::Dialogs, pages::dialogs::page)
    }
}

/// The Lab tab: slot accounting, stress, sandwich, the macOS reference.
pub struct LabTab(NativeWidgetsDemoState);

impl LabTab {
    /// The Lab tab over the shell's state.
    pub fn new(shell: &NativeWidgetsDemoState) -> Self {
        Self(shell.clone())
    }
}

impl Component for LabTab {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        self.0.clone()
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> impl View<NativeWidgetsDemoState> {
        NativeWidgetsDemoApp::tab_root(state, Tab::Lab, pages::lab::page)
    }
}

// The app's sole entry point: binds `NativeWidgetsDemoApp` to every platform.
// `setup` installs Glyph as the seeded default theme before any shell reads
// the default-theme slot (a shell's own fallback bundles no fonts for the
// native controls to adopt).
frust::app!(
    NativeWidgetsDemoApp,
    setup = {
        frust_glyph::install();
    }
);

#[cfg(test)]
mod deep_link_tests {
    use super::*;

    fn link(url: &str, sequence: u64) -> DeepLink {
        DeepLink {
            url: url.to_string(),
            sequence,
        }
    }

    fn go(tab: Tab, anchor: Option<&'static str>) -> Option<DeepLinkAction> {
        Some(DeepLinkAction::Go(Destination { tab, anchor }))
    }

    #[test]
    fn every_old_label_routes_to_its_tab_and_anchor_case_insensitively() {
        let expected = [
            ("Controls", Tab::Controls),
            ("macOS", Tab::Lab),
            ("New", Tab::Controls),
            ("Alerts", Tab::Dialogs),
            ("TabBar", Tab::Lab),
            ("Sheet", Tab::Dialogs),
            ("Composite", Tab::Controls),
            ("Stress", Tab::Lab),
            ("Sandwich", Tab::Lab),
        ];
        assert_eq!(expected.len(), SECTION_LABELS.len() + 1, "every old label");
        for (label, tab) in expected {
            for spelled in [
                label.to_string(),
                label.to_ascii_lowercase(),
                label.to_ascii_uppercase(),
            ] {
                let url = format!("native-widgets-demo://section/{spelled}");
                assert_eq!(
                    plan_deep_link(&link(&url, 1), None),
                    go(tab, Some(label)),
                    "{url}"
                );
            }
        }
    }

    #[test]
    fn every_tab_slug_routes_without_an_anchor() {
        for tab in Tab::ALL {
            for spelled in [tab.slug().to_string(), tab.title().to_ascii_uppercase()] {
                let url = format!("native-widgets-demo://tab/{spelled}");
                assert_eq!(plan_deep_link(&link(&url, 1), None), go(tab, None), "{url}");
            }
        }
    }

    #[test]
    fn tab_order_index_and_slug_round_trip() {
        for (index, tab) in Tab::ALL.into_iter().enumerate() {
            assert_eq!(tab.index(), index);
            assert_eq!(Tab::from_slug(tab.slug()), Some(tab));
        }
        assert_eq!(Tab::from_slug("profile"), None);
    }

    #[test]
    fn unknown_label_yields_unknown_label_action() {
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://section/nonexistent", 1), None),
            Some(DeepLinkAction::UnknownLabel("nonexistent".to_string()))
        );
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://tab/settings", 1), None),
            Some(DeepLinkAction::UnknownLabel("settings".to_string())),
            "an unknown tab slug"
        );
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://tab/macOS", 1), None),
            Some(DeepLinkAction::UnknownLabel("macOS".to_string())),
            "a section label is not a tab slug"
        );
    }

    #[test]
    fn wrong_scheme_or_host_is_ignored() {
        for url in [
            "frustplay://section/controls",
            "native-widgets-demo://other/controls",
            "native-widgets-demo://section",
            "native-widgets-demo://tab",
        ] {
            assert_eq!(plan_deep_link(&link(url, 1), None), None, "{url}");
        }
    }

    #[test]
    fn trailing_slash_query_and_fragment_still_resolve() {
        for url in [
            "native-widgets-demo://section/Stress/",
            "native-widgets-demo://section/Stress?count=50",
            "native-widgets-demo://section/Stress#top",
        ] {
            assert_eq!(
                plan_deep_link(&link(url, 1), None),
                go(Tab::Lab, Some("Stress")),
                "{url}"
            );
        }
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://tab/dialogs/?x#y", 1), None),
            go(Tab::Dialogs, None)
        );
    }

    #[test]
    fn repeated_identical_url_with_new_sequence_is_applied_again() {
        let first = link("native-widgets-demo://section/alerts", 1);
        assert!(plan_deep_link(&first, None).is_some());
        let second = link("native-widgets-demo://section/alerts", 2);
        assert!(
            plan_deep_link(&second, Some(first.sequence)).is_some(),
            "an identical URL with a new sequence must be applied again"
        );
    }

    #[test]
    fn same_sequence_is_not_reapplied() {
        let delivered = link("native-widgets-demo://section/alerts", 5);
        assert_eq!(plan_deep_link(&delivered, Some(delivered.sequence)), None);
    }

    #[test]
    fn empty_label_is_rejected() {
        assert_eq!(parse_deep_link("native-widgets-demo://section/"), None);
        assert_eq!(parse_deep_link("native-widgets-demo://tab/"), None);
    }

    #[test]
    fn env_value_selects_a_section_label_then_a_tab_slug() {
        let stress = Some(Destination {
            tab: Tab::Lab,
            anchor: Some("Stress"),
        });
        assert_eq!(destination_from_env_value(Some("stress")), stress);
        assert_eq!(destination_from_env_value(Some("  Stress\n")), stress);
        assert_eq!(
            destination_from_env_value(Some("controls")),
            Some(Destination {
                tab: Tab::Controls,
                anchor: Some("Controls"),
            }),
            "a section label wins over the tab slug it shares"
        );
        assert_eq!(
            destination_from_env_value(Some("Dialogs")),
            Some(Destination {
                tab: Tab::Dialogs,
                anchor: None,
            })
        );
        assert_eq!(destination_from_env_value(Some("nope")), None);
        assert_eq!(destination_from_env_value(Some("")), None);
        assert_eq!(destination_from_env_value(None), None);
    }

    #[test]
    fn detail_route_round_trips_through_its_location() {
        let route = DetailRoute::new("Rendering: damage-aware present", "frust-rs/frust");
        assert_eq!(DetailRoute::from_location(&route.location()), Some(route));
        let untitled = DetailRoute::new("Issues", "");
        assert_eq!(
            DetailRoute::from_location(&untitled.location()),
            Some(untitled)
        );
        assert_eq!(
            DetailRoute::from_location(&Location::parse("/other?title=x")),
            None
        );
    }

    #[test]
    fn deep_link_label_sanitizer_accepts_ascii_alnum_and_dash_within_the_length_cap() {
        assert_eq!(sanitize_deep_link_label("abc-123"), Some("abc-123"));
        assert_eq!(sanitize_deep_link_label("A-Z-0-9"), Some("A-Z-0-9"));
        let at_cap = "a".repeat(MAX_ECHOED_LABEL_LEN);
        assert_eq!(sanitize_deep_link_label(&at_cap), Some(at_cap.as_str()));
    }

    #[test]
    fn deep_link_label_sanitizer_rejects_empty_oversized_or_non_ascii_alnum_dash() {
        assert_eq!(sanitize_deep_link_label(""), None, "empty");
        let over_cap = "a".repeat(MAX_ECHOED_LABEL_LEN + 1);
        assert_eq!(
            sanitize_deep_link_label(&over_cap),
            None,
            "one over the cap"
        );
        assert_eq!(sanitize_deep_link_label("has space"), None, "space");
        assert_eq!(sanitize_deep_link_label("has/slash"), None, "slash");
        assert_eq!(sanitize_deep_link_label("<script>"), None, "markup");
        assert_eq!(sanitize_deep_link_label("caf\u{e9}"), None, "non-ASCII");
    }

    #[test]
    fn logged_label_short_ascii_is_escaped_whole() {
        assert_eq!(logged_deep_link_label("abc"), "\"abc\" (3 bytes)");
        // Control characters still reach the log escaped, never raw.
        assert_eq!(logged_deep_link_label("a\u{1}b"), "\"a\\u{1}b\" (3 bytes)");
    }

    #[test]
    fn logged_label_caps_by_characters_never_slicing_inside_one() {
        // 70 x 'é' is 140 bytes but 70 characters: under the cap, logged
        // whole. A byte slice of the escaped form at 128 would cut inside one.
        let seventy_e_acute: String = "\u{e9}".repeat(70);
        let logged = logged_deep_link_label(&seventy_e_acute);
        assert_eq!(logged, format!("{seventy_e_acute:?} (140 bytes)"));

        // 200 CJK characters (600 bytes): cut after 128 characters, marked.
        let cjk: String = "\u{65e5}".repeat(200);
        let logged = logged_deep_link_label(&cjk);
        let expected_prefix: String = "\u{65e5}".repeat(128);
        assert_eq!(logged, format!("{expected_prefix:?}... (600 bytes)"));

        // Plain ASCII over the cap is cut the same way.
        let long_ascii = "x".repeat(300);
        assert_eq!(
            logged_deep_link_label(&long_ascii),
            format!("{:?}... (300 bytes)", "x".repeat(128))
        );

        // Exactly at the cap: not marked.
        let at_cap = "y".repeat(128);
        assert_eq!(
            logged_deep_link_label(&at_cap),
            format!("{at_cap:?} (128 bytes)")
        );
    }

    #[test]
    fn append_toast_keeps_a_repeated_message_as_a_new_request() {
        // The host shows each log entry once, by index: a message equal to
        // the last one is a second request, not a duplicate to collapse.
        let mut queue: Vec<String> = Vec::new();
        append_toast(&mut queue, "same".to_string());
        append_toast(&mut queue, "same".to_string());
        append_toast(&mut queue, "different".to_string());
        append_toast(&mut queue, "different".to_string());
        assert_eq!(queue, vec!["same", "same", "different", "different"]);
    }

    #[test]
    fn push_toast_through_the_signal_appends_repeats_too() {
        let toasts: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
        push_toast(toasts, "same".to_string());
        push_toast(toasts, "same".to_string());
        assert_eq!(toasts.get_untracked(), vec!["same", "same"]);
    }

    #[test]
    fn append_toast_never_drains_from_the_front() {
        // The toast host has an append-only cursor, so draining from the front
        // shifts indices and causes messages to be skipped.
        let mut queue: Vec<String> = (0..8).map(|i| i.to_string()).collect();
        let before_len = queue.len();
        append_toast(&mut queue, "extra".to_string());
        assert_eq!(queue.len(), before_len + 1, "append-only, never shrinks");
        assert_eq!(queue.last().map(String::as_str), Some("extra"));
        assert_eq!(
            queue.first().map(String::as_str),
            Some("0"),
            "oldest entry '0' still present"
        );
    }

    #[test]
    fn rate_limit_decision_allows_first_unknown_toast() {
        let allow = should_rate_limit_unknown_toast(None, Instant::now());
        assert!(!allow, "first unknown toast is never rate-limited");
    }

    #[test]
    fn rate_limit_decision_suppresses_within_interval() {
        let now = Instant::now();
        let shortly_after = now + Duration::from_millis(500);
        let allow = should_rate_limit_unknown_toast(Some(now), shortly_after);
        assert!(
            allow,
            "unknown toast within the interval is rate-limited (suppressed)"
        );
    }

    #[test]
    fn rate_limit_decision_allows_after_interval() {
        let now = Instant::now();
        let after_interval = now + UNKNOWN_LABEL_TOAST_INTERVAL + Duration::from_millis(1);
        let allow = should_rate_limit_unknown_toast(Some(now), after_interval);
        assert!(
            !allow,
            "unknown toast after the interval is allowed (not suppressed)"
        );
    }

    /// A minimal model of the toast host's cursor: it clamps its cursor to the
    /// queue's length on every rebuild and consumes messages in order. Every
    /// push goes through the app's own [`append_toast`].
    struct ToastCursorModel {
        queue: Vec<String>,
        next_index: usize,
    }

    impl ToastCursorModel {
        fn new() -> Self {
            ToastCursorModel {
                queue: Vec::new(),
                next_index: 0,
            }
        }

        fn push(&mut self, message: &str) {
            append_toast(&mut self.queue, message.to_string());
        }

        fn clamp_cursor(&mut self) {
            self.next_index = self.next_index.min(self.queue.len());
        }

        fn take_next(&mut self) -> Option<String> {
            let msg = self.queue.get(self.next_index).cloned();
            if msg.is_some() {
                self.next_index += 1;
            }
            msg
        }
    }

    #[test]
    fn toast_cursor_model_consumes_every_message_exactly_once() {
        let mut model = ToastCursorModel::new();
        let mut consumed = Vec::new();
        for i in 0..4 {
            model.push(&i.to_string());
        }
        model.clamp_cursor();
        consumed.extend(model.take_next());
        assert_eq!(consumed, vec!["0"]);
        for i in 4..7 {
            model.push(&i.to_string());
        }
        model.clamp_cursor();
        while let Some(msg) = model.take_next() {
            consumed.push(msg);
        }
        for i in 7..10 {
            model.push(&i.to_string());
        }
        model.clamp_cursor();
        while let Some(msg) = model.take_next() {
            consumed.push(msg);
        }
        let expected: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        assert_eq!(
            consumed, expected,
            "every message consumed in order, none skipped"
        );
    }

    #[test]
    fn toast_cursor_model_shows_a_repeated_message_again() {
        // Push X, the host shows it, push X again: the host must show it again.
        let mut model = ToastCursorModel::new();
        let mut consumed = Vec::new();
        model.push("X");
        model.clamp_cursor();
        consumed.extend(model.take_next());
        model.push("X");
        model.clamp_cursor();
        consumed.extend(model.take_next());
        assert_eq!(consumed, vec!["X", "X"], "shown twice, once per request");
        assert_eq!(model.take_next(), None, "nothing pending afterwards");
    }
}
