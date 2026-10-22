//! App chrome's platform-free half (decision C1): the [`ChromeSpec`] data
//! contract every arm consumes, the [`ChromeEvent`] vocabulary that comes
//! back, and the process-wide publish slot between them.
//!
//! # A host-level surface, not a `platform_view` slot
//!
//! Tabs, title, leading item and actions belong to the host (iOS
//! `UITabBarController` and `UINavigationController`, Android
//! `MaterialToolbar` and `BottomNavigationView`), not to the differ, so one
//! `native_scaffold` per app publishes a spec here and an arm behind
//! [`ChromeHost`] owns the real containers. The bare `native_tab_bar` control is unchanged (D5): it
//! stays an embedded `platform_view` slot.
//!
//! # The slot
//!
//! - **Diff-gated.** The first [`publish`] installs; later ones diff against
//!   the retained spec ([`ChromeDiff`]) and apply only the changed fields; an
//!   identical spec makes no host call.
//! - **One publisher.** Only a generation-less publish installs, under a fresh, never-zero,
//!   increasing [`ChromeGeneration`]; naming another generation while one is live is refused
//!   [`ChromeError::Busy`], a withdrawn one on an empty slot is [`PublishOutcome::Stale`].
//!   [`withdraw`] (scaffold teardown or drop) frees the slot only for its own generation.
//! - **Controlled.** The bars never navigate: [`deliver`] decodes a
//!   `(kind, detail)` report against the live spec into a [`ChromeEvent`]
//!   for the registered sink, and the app confirms by publishing again.
//!   Reports stamped with a withdrawn generation are dropped, and so is a tap
//!   on a disabled tab or menu item.
//! - **Resolved where it was drawn.** An arm whose bars lag the slot (iOS
//!   applies on a later main-queue turn) resolves a tap against the spec it
//!   shows ([`resolve_tap`]) and hands the id-based event to [`deliver_event`],
//!   which re-encodes it against the live spec, so a reorder in between cannot
//!   retarget it and an item that is gone or disabled by then is dropped.
//! - **Failures reported late.** An arm that installs on a later turn reports
//!   a failure it finds there through [`ChromeHost::take_failure`]; the slot
//!   takes it at the top of the generation's next publish (or withdraw),
//!   answers that publish [`PublishOutcome::Fallback`] and, for
//!   [`ChromeError::NoHost`], retries the install on the one after.
//!
//! # The epoch
//!
//! An outcome change off the scaffold's build path (a failure an arm records
//! on the main queue, a scroll flush whose publish crosses between native and
//! fallback) calls [`bump_epoch`] outside the slot lock: it counts the change
//! and owes the shell a frame, whose rebuild re-runs the scaffold's sync so
//! its fallback bars and `native` flag follow. It is not a signal: every
//! frame that runs rebuilds the whole tree, so the frame is all that is owed,
//! and a signal the plugin created inside a build would die with that
//! component's owner.
//!
//! The module is behind `frust-api` because the spec reuses the api layer's
//! [`TabItem`]/[`TabIcon`]/[`TabId`]; it compiles on every target. iOS
//! installs `crate::apple::chrome`'s host and Android `crate::android::chrome`'s
//! (over the [`android_wire`] encoding); every other target uses the inert
//! `NullChromeHost`, and tests use `RecordingChromeHost`.

/// The frust-drawn bars the scaffold shows from the same spec wherever no arm
/// owns the chrome (see [`NATIVE_CHROME_ARM`]).
pub(crate) mod fallback;
pub(crate) mod host;

#[cfg(test)]
pub(crate) use host::RecordingChromeHost;
pub(crate) use host::{ChromeDiff, ChromeError, ChromeHost};

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};

use crate::api::{TabIcon, TabId, TabItem};
use crate::events::{
    EVENT_KIND_CHROME_ACTION, EVENT_KIND_CHROME_BACK, EVENT_KIND_CHROME_MENU,
    EVENT_KIND_CHROME_TAB, pack_chrome_menu, pack_chrome_tab, pack_index, unpack_chrome_menu,
    unpack_chrome_tab, unpack_index,
};

/// Whether this target has a native chrome arm: iOS and Android, so every
/// other target's scaffold draws the [`fallback`] bars. A target with an arm
/// still draws the fallback for a [`PublishOutcome::Fallback`] (its install
/// was refused — on Android, a theme that is not Material-derived).
pub(crate) const NATIVE_CHROME_ARM: bool = cfg!(any(target_os = "ios", target_os = "android"));

#[cfg(test)]
thread_local! {
    /// [`native_arm`]'s per-test-thread override.
    static TEST_ARM: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// [`NATIVE_CHROME_ARM`], which a test may override on its own thread
/// ([`ArmOverride`]) to drive the arm's outcome classes on a host build.
pub(crate) fn native_arm() -> bool {
    #[cfg(test)]
    if let Some(arm) = TEST_ARM.with(std::cell::Cell::get) {
        return arm;
    }
    NATIVE_CHROME_ARM
}

/// Test-only: [`native_arm`] answers `arm` on this thread until dropped.
#[cfg(test)]
pub(crate) struct ArmOverride;

#[cfg(test)]
impl ArmOverride {
    pub(crate) fn new(arm: bool) -> Self {
        TEST_ARM.with(|cell| cell.set(Some(arm)));
        Self
    }
}

#[cfg(test)]
impl Drop for ArmOverride {
    fn drop(&mut self) {
        TEST_ARM.with(|cell| cell.set(None));
    }
}

static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Record an outcome change made off the scaffold's build path and owe the
/// shell a frame, whose rebuild re-syncs the scaffold (module doc, "The
/// epoch"). Callable from any thread; never with the slot locked.
pub(crate) fn bump_epoch() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
    frust::texture::request_frame();
}

/// How many times [`bump_epoch`] ran.
#[cfg(test)]
pub(crate) fn epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}

// --- the data contract ------------------------------------------------------

/// Everything the app chrome shows. Byte payloads compare by content, so a
/// spec rebuilt every frame from `include_bytes!` diffs as unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChromeSpec {
    pub(crate) tabs: Vec<TabItem>,
    /// The app-confirmed tab; an id no tab carries shows no selection.
    pub(crate) selected: Option<TabId>,
    pub(crate) top: Option<TopBarSpec>,
    pub(crate) scroll: Option<ScrollState>,
    /// The app theme's brightness (`crate::api::theme::is_dark`), which an
    /// arm styles its bars with; the fallback bars read the theme directly.
    pub(crate) dark: bool,
}

/// The top bar: title, optional subtitle, leading item and trailing actions.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TopBarSpec {
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) leading: Leading,
    pub(crate) actions: Vec<ActionItem>,
    /// iOS large-title display; ignored where the platform has none.
    pub(crate) large_title: bool,
}

/// The top bar's leading item.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Leading {
    /// Nothing.
    #[default]
    None,
    /// A back item, reported as [`ChromeEvent::BackPressed`].
    Back,
    /// Encoded image bytes (an avatar), compared by content.
    Image(Arc<[u8]>),
    /// An Apple SF Symbol name; Android has no symbol-by-name.
    Symbol(String),
}

/// A chrome action's stable, app-chosen identity — never an index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(String);

/// A pull-down menu item's stable, app-chosen identity — never an index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MenuItemId(String);

macro_rules! string_id {
    ($id:ident) => {
        impl $id {
            /// An id spelled `id`.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The id's spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $id {
            fn from(id: &str) -> Self {
                Self::new(id)
            }
        }

        impl From<String> for $id {
            fn from(id: String) -> Self {
                Self(id)
            }
        }

        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(ActionId);
string_id!(MenuItemId);

/// One trailing top-bar action; with a [`Self::menu`] it is a pull-down
/// whose choice reports [`ChromeEvent::MenuItemChosen`] instead of a tap.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionItem {
    pub(crate) id: ActionId,
    pub(crate) title: String,
    pub(crate) icon: Option<TabIcon>,
    pub(crate) menu: Option<Vec<MenuItem>>,
}

impl ActionItem {
    /// An action `id` titled `title`, with no icon and no menu.
    pub fn new(id: impl Into<ActionId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: None,
            menu: None,
        }
    }

    /// The action's icon; the title then becomes its accessibility label.
    pub fn icon(mut self, icon: TabIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Make this action a pull-down over `items`.
    pub fn menu(mut self, items: Vec<MenuItem>) -> Self {
        self.menu = Some(items);
        self
    }

    /// This action's id.
    pub fn id(&self) -> &ActionId {
        &self.id
    }
}

/// One item of an [`ActionItem`]'s pull-down menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub(crate) id: MenuItemId,
    pub(crate) title: String,
    pub(crate) icon: Option<TabIcon>,
    pub(crate) destructive: bool,
    pub(crate) enabled: bool,
}

impl MenuItem {
    /// An enabled, non-destructive item `id` titled `title`.
    pub fn new(id: impl Into<MenuItemId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: None,
            destructive: false,
            enabled: true,
        }
    }

    /// The item's icon.
    pub fn icon(mut self, icon: TabIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Whether the item is styled destructive. Default `false`.
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Whether the item can be chosen. Default `true`.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// This item's id.
    pub fn id(&self) -> &MenuItemId {
        &self.id
    }
}

/// Whether the host's bars float over the body (iOS reports them through the
/// window insets); Android's arm shrinks the surface between its bars and the
/// fallback bars are laid out beside the body, so neither floats.
#[cfg_attr(not(feature = "frust-api"), allow(dead_code))]
pub(crate) const BARS_FLOAT_OVER_BODY: bool = cfg!(target_os = "ios");

/// The body's scroll position, mirrored to the host so the bars can react
/// (scroll-edge appearance, lifted app bar). Logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollState {
    /// How far the body is scrolled from its top.
    pub offset: f64,
    /// The largest reachable offset.
    pub max_offset: f64,
    /// The visible extent along the scroll axis.
    pub viewport: f64,
}

// --- the event vocabulary ---------------------------------------------------

/// One chrome interaction, decoded against the spec that was live when it
/// arrived. Requests, not facts: the app routes and republishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChromeEvent {
    /// A tab other than the showing one was tapped.
    TabSelected(TabId),
    /// The showing tab was tapped again ("scroll to top / pop to root").
    TabReselected(TabId),
    /// A plain action was tapped.
    ActionTapped(ActionId),
    /// An item of an action's pull-down menu was chosen.
    MenuItemChosen(ActionId, MenuItemId),
    /// The leading back item was tapped.
    BackPressed,
}

fn actions(spec: &ChromeSpec) -> &[ActionItem] {
    spec.top.as_ref().map_or(&[], |top| &top.actions)
}

/// `event` as the `(kind, detail)` an arm reports, resolving ids to the
/// first item carrying them in `spec`. `None` when `spec` has no such item.
pub(crate) fn encode_event(spec: &ChromeSpec, event: &ChromeEvent) -> Option<(i32, i64)> {
    let tab_index = |id: &TabId| spec.tabs.iter().position(|tab| tab.id() == id);
    let action_index = |id: &ActionId| actions(spec).iter().position(|action| &action.id == id);
    match event {
        ChromeEvent::TabSelected(id) => Some((
            EVENT_KIND_CHROME_TAB,
            pack_chrome_tab(tab_index(id)?, false)?,
        )),
        ChromeEvent::TabReselected(id) => Some((
            EVENT_KIND_CHROME_TAB,
            pack_chrome_tab(tab_index(id)?, true)?,
        )),
        ChromeEvent::ActionTapped(id) => Some((
            EVENT_KIND_CHROME_ACTION,
            pack_index(action_index(id)? as isize),
        )),
        ChromeEvent::MenuItemChosen(action, item) => {
            let a = action_index(action)?;
            let menu = actions(spec)[a].menu.as_deref()?;
            let m = menu.iter().position(|entry| &entry.id == item)?;
            Some((EVENT_KIND_CHROME_MENU, pack_chrome_menu(a, m)?))
        }
        ChromeEvent::BackPressed => Some((EVENT_KIND_CHROME_BACK, 0)),
    }
}

/// [`encode_event`]'s inverse: `None` for an unknown kind, a malformed
/// `detail` or an index `spec` does not have.
pub(crate) fn decode_event(spec: &ChromeSpec, kind: i32, detail: i64) -> Option<ChromeEvent> {
    match kind {
        EVENT_KIND_CHROME_TAB => {
            let (index, reselect) = unpack_chrome_tab(detail)?;
            let tab = spec.tabs.get(index).filter(|tab| tab.is_enabled())?;
            let id = tab.id().clone();
            Some(if reselect {
                ChromeEvent::TabReselected(id)
            } else {
                ChromeEvent::TabSelected(id)
            })
        }
        EVENT_KIND_CHROME_ACTION => {
            let action = actions(spec).get(unpack_index(detail)?)?;
            Some(ChromeEvent::ActionTapped(action.id.clone()))
        }
        EVENT_KIND_CHROME_MENU => {
            let (a, m) = unpack_chrome_menu(detail)?;
            let action = actions(spec).get(a)?;
            let item = action.menu.as_ref()?.get(m).filter(|item| item.enabled)?;
            Some(ChromeEvent::MenuItemChosen(
                action.id.clone(),
                item.id.clone(),
            ))
        }
        EVENT_KIND_CHROME_BACK => (detail == 0).then_some(ChromeEvent::BackPressed),
        _ => None,
    }
}

/// One tap on an arm's on-screen chrome, by position in the spec it was
/// built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) enum ChromeTap {
    /// The tab at this index of `spec.tabs`.
    Tab(usize),
    /// The trailing action at this index.
    Action(usize),
    /// Item `.1` of action `.0`'s pull-down menu.
    Menu(usize, usize),
    /// The leading back item.
    Back,
}

/// What `tap` requests, against the `spec` the tapped bar was built from: a
/// tab tap is a reselect when it lands on the app-confirmed `selected` tab
/// and a selection otherwise. `None` when `spec` has no such item, or it is
/// disabled, or the back tap meets no back item.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn resolve_tap(spec: &ChromeSpec, tap: ChromeTap) -> Option<ChromeEvent> {
    let (kind, detail) = match tap {
        ChromeTap::Tab(index) => {
            let reselect = spec.selected.as_ref() == Some(spec.tabs.get(index)?.id());
            (EVENT_KIND_CHROME_TAB, pack_chrome_tab(index, reselect)?)
        }
        ChromeTap::Action(index) => (
            EVENT_KIND_CHROME_ACTION,
            pack_index(isize::try_from(index).ok()?),
        ),
        ChromeTap::Menu(action, item) => (EVENT_KIND_CHROME_MENU, pack_chrome_menu(action, item)?),
        ChromeTap::Back => {
            let top = spec.top.as_ref()?;
            (top.leading == Leading::Back).then_some((EVENT_KIND_CHROME_BACK, 0))?
        }
    };
    decode_event(spec, kind, detail)
}

// --- the slot ---------------------------------------------------------------

/// One install of the app chrome; never zero, increasing per slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ChromeGeneration(u64);

impl ChromeGeneration {
    /// The raw value an arm stamps on its reports.
    pub(crate) fn get(self) -> u64 {
        self.0
    }
}

/// Where decoded events go — set by the scaffold, called outside the slot
/// lock so it may publish again.
pub(crate) type ChromeSink = Arc<dyn Fn(ChromeEvent) + Send + Sync>;

/// What one [`publish`] did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PublishOutcome {
    /// A fresh generation claimed the slot and the host installed it.
    Installed(ChromeGeneration),
    /// Changed fields were applied to the host.
    Applied(ChromeGeneration),
    /// The spec was identical; no host call.
    Unchanged(ChromeGeneration),
    /// The generation holds the slot but its host refused the install, so
    /// the app renders fallback chrome; no host call for this generation.
    Fallback(ChromeGeneration, ChromeError),
    /// Another generation holds the slot ([`ChromeError::Busy`]).
    Refused(ChromeError),
    /// The publisher's generation was withdrawn and the slot is empty; nothing
    /// was installed. Only a publisher holding no generation may claim it.
    Stale,
}

impl PublishOutcome {
    /// The generation that holds the slot for this publisher, if any.
    pub(crate) fn generation(&self) -> Option<ChromeGeneration> {
        match self {
            Self::Installed(generation)
            | Self::Applied(generation)
            | Self::Unchanged(generation)
            | Self::Fallback(generation, _) => Some(*generation),
            Self::Refused(_) | Self::Stale => None,
        }
    }
}

struct Live {
    generation: ChromeGeneration,
    spec: ChromeSpec,
    /// `Err` when the host refused the install.
    installed: Result<(), ChromeError>,
    sink: Option<ChromeSink>,
    busy_logged: bool,
}

/// The slot over one host. The process-wide instance is behind [`publish`]
/// and friends; tests build their own over a [`RecordingChromeHost`].
pub(crate) struct ChromeSlot<H> {
    host: H,
    live: Option<Live>,
    last_generation: u64,
}

impl<H: ChromeHost> ChromeSlot<H> {
    pub(crate) const fn new(host: H) -> Self {
        Self {
            host,
            live: None,
            last_generation: 0,
        }
    }

    /// Publish `spec` as `publisher` — `None` for a scaffold that holds no
    /// generation yet, else the one its first publish returned. A generation
    /// whose install failed [`ChromeError::NoHost`] retries it here.
    pub(crate) fn publish(
        &mut self,
        publisher: Option<ChromeGeneration>,
        spec: ChromeSpec,
    ) -> PublishOutcome {
        let Some(live) = self.live.as_mut() else {
            if publisher.is_some() {
                return PublishOutcome::Stale;
            }
            self.last_generation = self.last_generation.checked_add(1).unwrap_or(1);
            let generation = ChromeGeneration(self.last_generation);
            let installed = self.host.install(generation, &spec);
            let outcome = match &installed {
                Ok(()) => PublishOutcome::Installed(generation),
                Err(error) => {
                    log_install_failure(error);
                    PublishOutcome::Fallback(generation, error.clone())
                }
            };
            self.live = Some(Live {
                generation,
                spec,
                installed,
                sink: None,
                busy_logged: false,
            });
            return outcome;
        };
        if publisher != Some(live.generation) {
            if !live.busy_logged {
                live.busy_logged = true;
                log::warn!(
                    "frust-native-widgets: native_scaffold refused — chrome generation {} is \
                     live (one native_scaffold per app)",
                    live.generation.0
                );
            }
            return PublishOutcome::Refused(ChromeError::Busy);
        }
        let generation = live.generation;
        if absorb_failure(&mut self.host, live)
            && let Err(error) = &live.installed
        {
            // Fresh from the arm: fall back now, retry (NoHost) on the next publish.
            let error = error.clone();
            live.spec = spec;
            return PublishOutcome::Fallback(generation, error);
        }
        if live.installed == Err(ChromeError::NoHost) {
            // NoHost is transient: retry; a success switches to the arm.
            live.installed = self.host.install(generation, &spec);
            live.spec = spec;
            return match &live.installed {
                Ok(()) => PublishOutcome::Installed(generation),
                Err(error) => {
                    if *error != ChromeError::NoHost {
                        log_install_failure(error);
                    }
                    PublishOutcome::Fallback(generation, error.clone())
                }
            };
        }
        if let Err(error) = &live.installed {
            let error = error.clone();
            live.spec = spec;
            return PublishOutcome::Fallback(generation, error);
        }
        let diff = ChromeDiff::between(&live.spec, &spec);
        if diff.is_empty() {
            return PublishOutcome::Unchanged(generation);
        }
        self.host.apply(&diff);
        live.spec = spec;
        PublishOutcome::Applied(generation)
    }

    /// Free the slot if `generation` holds it, disposing an installed host.
    /// `false` (and nothing done) for any other generation.
    pub(crate) fn withdraw(&mut self, generation: ChromeGeneration) -> bool {
        if self.live.as_ref().map(|live| live.generation) != Some(generation) {
            return false;
        }
        if let Some(live) = self.live.as_mut() {
            absorb_failure(&mut self.host, live);
        }
        self.release();
        true
    }

    /// Drop the live generation: dispose an installed host, else tell it the
    /// failed generation is gone.
    fn release(&mut self) {
        match self.live.take() {
            Some(Live {
                installed: Ok(()), ..
            }) => self.host.dispose(),
            Some(live) => self.host.abandon(live.generation),
            None => {}
        }
    }

    /// Test-only: drop whatever is live, disposing an installed host.
    #[cfg(test)]
    fn clear(&mut self) {
        if let Some(live) = self.live.as_mut() {
            absorb_failure(&mut self.host, live);
        }
        self.release();
    }

    /// Register where `generation`'s events go; `false` if it is not live.
    pub(crate) fn set_sink(&mut self, generation: ChromeGeneration, sink: ChromeSink) -> bool {
        match self.live.as_mut() {
            Some(live) if live.generation == generation => {
                live.sink = Some(sink);
                true
            }
            _ => false,
        }
    }

    /// Decode a report stamped `generation` against the live spec, with the
    /// sink to hand it to. `None` for a withdrawn generation, an undecodable
    /// report or no sink.
    pub(crate) fn route(
        &self,
        generation: u64,
        kind: i32,
        detail: i64,
    ) -> Option<(ChromeEvent, ChromeSink)> {
        let live = self
            .live
            .as_ref()
            .filter(|live| live.generation.0 == generation)?;
        let event = decode_event(&live.spec, kind, detail)?;
        Some((event, Arc::clone(live.sink.as_ref()?)))
    }

    /// [`Self::route`] for an id-based `event` an arm resolved against the
    /// spec it shows: re-encoded against the live spec first, so it reaches
    /// the sink only while that spec still carries the same, enabled item.
    #[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
    pub(crate) fn route_event(
        &self,
        generation: u64,
        event: &ChromeEvent,
    ) -> Option<(ChromeEvent, ChromeSink)> {
        let live = self
            .live
            .as_ref()
            .filter(|live| live.generation.0 == generation)?;
        let (kind, detail) = encode_event(&live.spec, event)?;
        self.route(generation, kind, detail)
    }

    /// The live generation, if any.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> Option<ChromeGeneration> {
        self.live.as_ref().map(|live| live.generation)
    }

    /// The retained spec of the live generation, if any.
    #[cfg(test)]
    pub(crate) fn spec(&self) -> Option<&ChromeSpec> {
        self.live.as_ref().map(|live| &live.spec)
    }

    #[cfg(test)]
    pub(crate) fn host(&self) -> &H {
        &self.host
    }

    #[cfg(test)]
    pub(crate) fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }
}

/// Take a failure `host` reported for `live` since it answered the install
/// `Ok` (the arm logged it); `true` when one arrived.
fn absorb_failure<H: ChromeHost>(host: &mut H, live: &mut Live) -> bool {
    let Some(error) = host.take_failure(live.generation) else {
        return false;
    };
    live.installed = Err(error);
    true
}

/// Log an arm's failed install. A refusal is a property of the app's theme,
/// not of one scaffold, so it is logged once per process and names the fix.
fn log_install_failure(error: &ChromeError) {
    static REFUSAL_LOGGED: Once = Once::new();
    match error {
        ChromeError::Refused(_) => REFUSAL_LOGGED.call_once(|| {
            log::warn!(
                "frust-native-widgets: {error}; the native chrome arm needs a Material3-derived \
                 app theme (Android: a Theme.Material3.* parent) — rendering frust-drawn \
                 fallback chrome"
            );
        }),
        _ => log::warn!("frust-native-widgets: {error}; rendering fallback chrome"),
    }
}

/// The host behind the process-wide slot, per target: the iOS and Android
/// arms, the recorder under test, and the inert host everywhere else.
#[cfg(all(target_os = "ios", not(test)))]
type PlatformChromeHost = crate::apple::IosChromeHost;
#[cfg(all(target_os = "android", not(test)))]
type PlatformChromeHost = crate::android::AndroidChromeHost;
#[cfg(test)]
type PlatformChromeHost = RecordingChromeHost;
#[cfg(not(any(test, target_os = "ios", target_os = "android")))]
type PlatformChromeHost = host::NullChromeHost;

static SLOT: Mutex<ChromeSlot<PlatformChromeHost>> =
    Mutex::new(ChromeSlot::new(PlatformChromeHost::new()));

/// Lock [`SLOT`], recovering from poisoning: a panicking caller must not turn
/// every later chrome call into a panic.
fn lock_slot() -> MutexGuard<'static, ChromeSlot<PlatformChromeHost>> {
    SLOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serialises every test that touches the process-wide slot and hands it
/// over clean, so a test that panicked while holding it cannot leave a
/// generation live for the next. Poisoning is ignored for the same reason.
#[cfg(test)]
pub(crate) fn slot_test_guard() -> MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    let guard = GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut slot = lock_slot();
    slot.clear();
    *slot.host_mut() = RecordingChromeHost::new();
    guard
}

/// Test-only: run `f` on the process-wide slot's recording host.
#[cfg(test)]
pub(crate) fn with_test_host<R>(f: impl FnOnce(&mut RecordingChromeHost) -> R) -> R {
    f(lock_slot().host_mut())
}

/// Test-only: the process-wide slot's live generation, if any.
#[cfg(test)]
pub(crate) fn live_generation() -> Option<ChromeGeneration> {
    lock_slot().generation()
}

/// Test-only: the process-wide slot's retained spec, if a generation is live.
#[cfg(test)]
pub(crate) fn live_spec() -> Option<ChromeSpec> {
    lock_slot().spec().cloned()
}

/// Publish `spec` to the process-wide slot — see [`ChromeSlot::publish`].
pub(crate) fn publish(publisher: Option<ChromeGeneration>, spec: ChromeSpec) -> PublishOutcome {
    lock_slot().publish(publisher, spec)
}

/// Scaffold teardown — see [`ChromeSlot::withdraw`].
pub(crate) fn withdraw(generation: ChromeGeneration) -> bool {
    lock_slot().withdraw(generation)
}

/// Register `generation`'s event sink — see [`ChromeSlot::set_sink`].
pub(crate) fn set_sink(generation: ChromeGeneration, sink: ChromeSink) -> bool {
    lock_slot().set_sink(generation, sink)
}

/// An arm's report: decode it and hand it to the sink, outside the lock.
/// Answers whether it was delivered.
pub(crate) fn deliver(generation: u64, kind: i32, detail: i64) -> bool {
    let routed = lock_slot().route(generation, kind, detail);
    hand_over(routed)
}

/// An arm's already-resolved report — see [`ChromeSlot::route_event`].
/// Answers whether it was delivered.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn deliver_event(generation: u64, event: &ChromeEvent) -> bool {
    let routed = lock_slot().route_event(generation, event);
    hand_over(routed)
}

/// Call the sink with the routed event; the slot lock is already released.
fn hand_over(routed: Option<(ChromeEvent, ChromeSink)>) -> bool {
    match routed {
        Some((event, sink)) => {
            sink(event);
            true
        }
        None => false,
    }
}

// --- the Android arm's wire -------------------------------------------------

/// The Android arm's wire (decision C3): the flat JSON
/// `FrustNativeChrome.install`/`apply` read and the revision-gated icon table
/// whose bytes travel through `setIcon` instead. Pure, so it compiles and is
/// host-tested on every target; `crate::android::chrome` is its one
/// production caller.
///
/// # The contract
///
/// `FrustNativeChrome.kt`'s file header is the authority: the keys, the
/// `changed` array's [`ChromeField`] spellings, the badge encoding (absent or
/// `null` = none, `""` = dot, anything else shown as-is — Kotlin turns a
/// numeric string into a count) and the `ICON_SLOT_*` codes.
/// `tests/kotlin_conformance.rs` pins the copy of that header the tests below
/// round-trip against, and the slot codes.
///
/// # What `apply` sends
///
/// Only the changed fields' keys, plus a `changed` array naming them —
/// except [`ChromeField::Scroll`], which is never JSON (the arm calls
/// `setScroll` directly), and except that a [`ChromeField::Tabs`] change
/// also carries [`ChromeField::Selected`]: the wire names the selection by
/// index, so a reorder that keeps the selected id moves its index.
///
/// # Icons
///
/// [`IconTable`] is `controls::tab_bar`'s icon side table in miniature: one
/// entry per `(slot, index)` the arm has handed Kotlin, re-sent only when the
/// bytes change (compared by content, like the spec), and a revision per
/// entry that `tab<i>IconRev` carries. Sync it before encoding: the JSON
/// reads its revisions. An Apple symbol has no Android rendering — a tab or
/// action carrying one is title-only and a `Leading::Symbol` encodes as
/// `"none"` (limitation `native-chrome-android-bitmap-icons`).
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) mod android_wire {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use super::host::ChromeField;
    use super::{ChromeSpec, Leading, TopBarSpec};
    use crate::api::TabIcon;
    use crate::runtime::escape;

    /// `setIcon` slot: a bottom tab's icon, `index` = tab index.
    pub(crate) const ICON_SLOT_TAB: i32 = 0;
    /// `setIcon` slot: a top-bar action's icon, `index` = action index.
    pub(crate) const ICON_SLOT_ACTION: i32 = 1;
    /// `setIcon` slot: the leading image (avatar), `index` `0`.
    pub(crate) const ICON_SLOT_LEADING: i32 = 2;

    /// Every field `install` encodes, in [`ChromeField`] declaration order.
    pub(crate) const INSTALL_FIELDS: [ChromeField; 9] = [
        ChromeField::Tabs,
        ChromeField::Selected,
        ChromeField::TopBar,
        ChromeField::Title,
        ChromeField::Subtitle,
        ChromeField::Leading,
        ChromeField::Actions,
        ChromeField::LargeTitle,
        ChromeField::Brightness,
    ];

    /// `field`'s spelling in an apply's `changed` array — its Rust name.
    pub(crate) fn field_name(field: ChromeField) -> &'static str {
        match field {
            ChromeField::Tabs => "Tabs",
            ChromeField::Selected => "Selected",
            ChromeField::TopBar => "TopBar",
            ChromeField::Title => "Title",
            ChromeField::Subtitle => "Subtitle",
            ChromeField::Leading => "Leading",
            ChromeField::Actions => "Actions",
            ChromeField::LargeTitle => "LargeTitle",
            ChromeField::Scroll => "Scroll",
            ChromeField::Brightness => "Brightness",
        }
    }

    /// One JSON value of the flat object.
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) enum Value {
        Str(String),
        /// `null` for `None`.
        OptStr(Option<String>),
        Bool(bool),
        Int(i64),
    }

    /// The flat object's entries, in encoding order.
    pub(crate) type Fields = Vec<(String, Value)>;

    /// The `install` payload: every key, no `changed` array.
    pub(crate) fn install_json(spec: &ChromeSpec, icons: &IconTable) -> String {
        to_json(&fields(spec, &INSTALL_FIELDS, icons), None)
    }

    /// The `apply` payload for a diff whose changed fields are `changed`, or
    /// `None` when none of them travels as JSON (a scroll-only diff).
    pub(crate) fn apply_json(
        spec: &ChromeSpec,
        changed: &[ChromeField],
        icons: &IconTable,
    ) -> Option<String> {
        let wire = wire_fields(changed);
        if wire.is_empty() {
            return None;
        }
        Some(to_json(&fields(spec, &wire, icons), Some(&wire)))
    }

    /// The fields an apply names on the wire for `changed`: no
    /// [`ChromeField::Scroll`], and [`ChromeField::Selected`] beside a
    /// [`ChromeField::Tabs`] change (module doc), in declaration order.
    pub(crate) fn wire_fields(changed: &[ChromeField]) -> Vec<ChromeField> {
        let tabs = changed.contains(&ChromeField::Tabs);
        INSTALL_FIELDS
            .into_iter()
            .filter(|field| changed.contains(field) || (tabs && *field == ChromeField::Selected))
            .collect()
    }

    /// The entries `wire` names, read from `spec` (an absent top bar reads as
    /// an empty one, `hasTop` false).
    pub(crate) fn fields(spec: &ChromeSpec, wire: &[ChromeField], icons: &IconTable) -> Fields {
        let empty = TopBarSpec::default();
        let top = spec.top.as_ref().unwrap_or(&empty);
        let mut out = Fields::new();
        let mut put = |key: String, value: Value| out.push((key, value));
        for field in wire {
            match field {
                ChromeField::Tabs => {
                    put("tabCount".into(), Value::Int(count(spec.tabs.len())));
                    for (i, tab) in spec.tabs.iter().enumerate() {
                        put(format!("tab{i}Title"), Value::Str(tab.title().into()));
                        put(format!("tab{i}Enabled"), Value::Bool(tab.is_enabled()));
                        put(
                            format!("tab{i}Badge"),
                            Value::OptStr(tab.badge_text().map(str::to_owned)),
                        );
                        let rev = icons.rev(ICON_SLOT_TAB, i);
                        put(format!("tab{i}IconRev"), Value::Int(rev_value(rev)));
                    }
                }
                ChromeField::Selected => {
                    put("selectedIndex".into(), Value::Int(selected_index(spec)));
                }
                ChromeField::TopBar => put("hasTop".into(), Value::Bool(spec.top.is_some())),
                ChromeField::Title => put("title".into(), Value::Str(top.title.clone())),
                ChromeField::Subtitle => {
                    put("subtitle".into(), Value::OptStr(top.subtitle.clone()));
                }
                ChromeField::Leading => {
                    put(
                        "leading".into(),
                        Value::Str(leading_name(&top.leading).into()),
                    );
                }
                ChromeField::Actions => {
                    put("actionCount".into(), Value::Int(count(top.actions.len())));
                    for (a, action) in top.actions.iter().enumerate() {
                        let menu = action.menu.as_deref().unwrap_or_default();
                        put(format!("action{a}Title"), Value::Str(action.title.clone()));
                        put(
                            format!("action{a}HasIcon"),
                            Value::Bool(matches!(action.icon, Some(TabIcon::Bytes(_)))),
                        );
                        put(format!("action{a}MenuCount"), Value::Int(count(menu.len())));
                        for (m, item) in menu.iter().enumerate() {
                            put(format!("menu{a}_{m}Title"), Value::Str(item.title.clone()));
                            put(
                                format!("menu{a}_{m}Destructive"),
                                Value::Bool(item.destructive),
                            );
                            put(format!("menu{a}_{m}Enabled"), Value::Bool(item.enabled));
                        }
                    }
                }
                ChromeField::LargeTitle => {
                    put("largeTitle".into(), Value::Bool(top.large_title));
                }
                ChromeField::Scroll => {}
                ChromeField::Brightness => put("dark".into(), Value::Bool(spec.dark)),
            }
        }
        out
    }

    /// `leading`'s wire spelling: an Apple symbol has no Android rendering.
    pub(crate) fn leading_name(leading: &Leading) -> &'static str {
        match leading {
            Leading::None | Leading::Symbol(_) => "none",
            Leading::Back => "back",
            Leading::Image(_) => "image",
        }
    }

    /// The selected tab's index, `-1` for none (an id no tab carries).
    fn selected_index(spec: &ChromeSpec) -> i64 {
        spec.selected
            .as_ref()
            .and_then(|id| spec.tabs.iter().position(|tab| tab.id() == id))
            .map_or(-1, count)
    }

    fn count(n: usize) -> i64 {
        i64::try_from(n).unwrap_or(i64::MAX)
    }

    fn rev_value(rev: u64) -> i64 {
        i64::try_from(rev).unwrap_or(i64::MAX)
    }

    /// Serialize `fields` as one flat JSON object, with a `changed` array
    /// when it is an apply. Keys are built from ASCII and never escaped.
    pub(crate) fn to_json(fields: &Fields, changed: Option<&[ChromeField]>) -> String {
        let mut out = String::with_capacity(32 + fields.len() * 24);
        out.push('{');
        for (i, (key, value)) in fields.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "\"{key}\":");
            match value {
                Value::Str(text) | Value::OptStr(Some(text)) => {
                    let _ = write!(out, "\"{}\"", escape(text));
                }
                Value::OptStr(None) => out.push_str("null"),
                Value::Bool(flag) => {
                    let _ = write!(out, "{flag}");
                }
                Value::Int(n) => {
                    let _ = write!(out, "{n}");
                }
            }
        }
        if let Some(changed) = changed {
            if !fields.is_empty() {
                out.push(',');
            }
            out.push_str("\"changed\":[");
            for (i, field) in changed.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "\"{}\"", field_name(*field));
            }
            out.push(']');
        }
        out.push('}');
        out
    }

    /// One `setIcon(slot, index, bytes)` call: `None` clears the entry
    /// (empty bytes on the wire).
    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct IconUpload {
        pub(crate) slot: i32,
        pub(crate) index: usize,
        pub(crate) bytes: Option<Arc<[u8]>>,
    }

    #[derive(Debug)]
    struct SentIcon {
        slot: i32,
        index: usize,
        rev: u64,
        bytes: Arc<[u8]>,
    }

    /// The icons handed to Kotlin, by `(slot, index)` — module doc's *Icons*.
    #[derive(Debug)]
    pub(crate) struct IconTable {
        sent: Vec<SentIcon>,
        next_rev: u64,
    }

    impl Default for IconTable {
        fn default() -> Self {
            Self::new()
        }
    }

    impl IconTable {
        /// An empty table (const, for the host in the slot's `static`).
        pub(crate) const fn new() -> Self {
            Self {
                sent: Vec::new(),
                next_rev: 1,
            }
        }

        /// The revision the bytes at `(slot, index)` were sent at; `0` for
        /// none.
        pub(crate) fn rev(&self, slot: i32, index: usize) -> u64 {
            self.sent
                .iter()
                .find(|sent| sent.slot == slot && sent.index == index)
                .map_or(0, |sent| sent.rev)
        }

        /// Forget every entry — Kotlin's cache is gone (`dispose`) or about
        /// to be refilled from scratch (`install`).
        pub(crate) fn clear(&mut self) {
            self.sent.clear();
        }

        /// Bring the table to `spec`'s byte icons, answering the calls that
        /// get Kotlin there: clears for entries `spec` no longer has, then
        /// new or changed bytes, in slot then index order. Unchanged bytes
        /// send nothing.
        pub(crate) fn sync(&mut self, spec: &ChromeSpec) -> Vec<IconUpload> {
            let wanted = wanted_icons(spec);
            let mut uploads = Vec::new();
            self.sent.retain(|sent| {
                let keep = wanted
                    .iter()
                    .any(|(slot, index, _)| *slot == sent.slot && *index == sent.index);
                if !keep {
                    uploads.push(IconUpload {
                        slot: sent.slot,
                        index: sent.index,
                        bytes: None,
                    });
                }
                keep
            });
            for (slot, index, bytes) in wanted {
                let rev = self.next_rev;
                let entry = self
                    .sent
                    .iter_mut()
                    .find(|sent| sent.slot == slot && sent.index == index);
                match entry {
                    Some(sent) if *sent.bytes == **bytes => continue,
                    Some(sent) => {
                        sent.bytes = Arc::clone(bytes);
                        sent.rev = rev;
                    }
                    None => self.sent.push(SentIcon {
                        slot,
                        index,
                        rev,
                        bytes: Arc::clone(bytes),
                    }),
                }
                self.next_rev += 1;
                uploads.push(IconUpload {
                    slot,
                    index,
                    bytes: Some(Arc::clone(bytes)),
                });
            }
            uploads
        }
    }

    /// Every byte icon `spec` shows, as `(slot, index, bytes)`: tab icons
    /// (the unselected one — Kotlin's bottom bar has one icon per item),
    /// action icons and the leading image.
    fn wanted_icons(spec: &ChromeSpec) -> Vec<(i32, usize, &Arc<[u8]>)> {
        let mut wanted = Vec::new();
        for (i, tab) in spec.tabs.iter().enumerate() {
            if let TabIcon::Bytes(bytes) = tab.icon() {
                wanted.push((ICON_SLOT_TAB, i, bytes));
            }
        }
        if let Some(top) = &spec.top {
            for (a, action) in top.actions.iter().enumerate() {
                if let Some(TabIcon::Bytes(bytes)) = &action.icon {
                    wanted.push((ICON_SLOT_ACTION, a, bytes));
                }
            }
            if let Leading::Image(bytes) = &top.leading {
                wanted.push((ICON_SLOT_LEADING, 0, bytes));
            }
        }
        wanted
    }

    #[cfg(test)]
    mod tests {
        use std::collections::BTreeSet;

        use super::*;
        use crate::api::{TabId, TabItem};
        use crate::chrome::{ActionItem, MenuItem};

        /// `FrustNativeChrome.kt`'s key table, copied verbatim from its file
        /// header (`tests/kotlin_conformance.rs` fails if the two drift).
        const KOTLIN_KEY_HEADER: &str = r#"
//   Tabs:     tabCount, tab<i>Title, tab<i>Enabled, tab<i>Badge (absent/null =
//             none, "" = dot), tab<i>IconRev (0 = no icon)
//   Selected: selectedIndex (-1 = none)
//   TopBar:   hasTop          Title: title          Subtitle: subtitle (absent/null = none)
//   Leading:  leading = "none" | "back" | "image"   LargeTitle: largeTitle (ignored here)
//   Actions:  actionCount, action<i>Title, action<i>HasIcon, action<i>MenuCount,
//             menu<i>_<j>Title, menu<i>_<j>Destructive, menu<i>_<j>Enabled
//   Brightness: dark
"#;

        /// Every key template [`KOTLIN_KEY_HEADER`] names.
        const KEY_TEMPLATES: [&str; 20] = [
            "tabCount",
            "tab<i>Title",
            "tab<i>Enabled",
            "tab<i>Badge",
            "tab<i>IconRev",
            "selectedIndex",
            "hasTop",
            "title",
            "subtitle",
            "leading",
            "largeTitle",
            "actionCount",
            "action<i>Title",
            "action<i>HasIcon",
            "action<i>MenuCount",
            "menu<i>_<j>Title",
            "menu<i>_<j>Destructive",
            "menu<i>_<j>Enabled",
            "dark",
            // Not a spec key: an apply's field list.
            "changed",
        ];

        fn bytes(content: &[u8]) -> Arc<[u8]> {
            Arc::from(content.to_vec().into_boxed_slice())
        }

        /// A spec exercising every key: two tabs (one byte icon, one badge),
        /// a top bar with an icon action and a two-item menu action.
        fn full_spec() -> ChromeSpec {
            ChromeSpec {
                tabs: vec![
                    TabItem::new("home", "Home", TabIcon::Bytes(bytes(&[1]))).badge("3"),
                    TabItem::new("me", "Me", TabIcon::AppleSymbol("person".into())).enabled(false),
                ],
                selected: Some(TabId::new("me")),
                top: Some(TopBarSpec {
                    title: "Inbox".into(),
                    subtitle: Some("2 new".into()),
                    leading: Leading::Image(bytes(&[7])),
                    actions: vec![
                        ActionItem::new("add", "Add").icon(TabIcon::Bytes(bytes(&[2]))),
                        ActionItem::new("more", "More").menu(vec![
                            MenuItem::new("share", "Share"),
                            MenuItem::new("delete", "Delete")
                                .destructive(true)
                                .enabled(false),
                        ]),
                    ],
                    large_title: true,
                }),
                scroll: None,
                dark: true,
            }
        }

        /// `key` with its first digit run spelled `<i>` and its second `<j>`.
        fn template(key: &str) -> String {
            let mut out = String::new();
            let mut runs = 0;
            let mut in_run = false;
            for c in key.chars() {
                if c.is_ascii_digit() {
                    if !in_run {
                        out.push_str(if runs == 0 { "<i>" } else { "<j>" });
                        runs += 1;
                        in_run = true;
                    }
                } else {
                    in_run = false;
                    out.push(c);
                }
            }
            out
        }

        /// Whether `token` appears in `text` as a whole word.
        fn names(text: &str, token: &str) -> bool {
            text.match_indices(token).any(|(at, _)| {
                let word = |c: char| c.is_ascii_alphanumeric() || c == '<' || c == '>';
                let before = text[..at].chars().next_back().is_none_or(|c| !word(c));
                let after = text[at + token.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !word(c) && c != '_');
                before && after
            })
        }

        /// The JSON keys of a flat object as the test parser sees them:
        /// every string literal followed by `:`, honouring escapes.
        fn json_keys(json: &str) -> Vec<String> {
            let mut keys = Vec::new();
            let mut chars = json.chars().peekable();
            while let Some(c) = chars.next() {
                if c != '"' {
                    continue;
                }
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        c => text.push(c),
                    }
                }
                if chars.peek() == Some(&':') {
                    keys.push(text);
                }
            }
            keys
        }

        #[test]
        fn install_round_trips_the_kotlin_key_table() {
            for template in KEY_TEMPLATES {
                assert!(
                    template == "changed" || names(KOTLIN_KEY_HEADER, template),
                    "{template} is not in the Kotlin header copy"
                );
            }
            let mut icons = IconTable::new();
            icons.sync(&full_spec());
            let json = install_json(&full_spec(), &icons);
            let encoded: BTreeSet<String> = json_keys(&json).iter().map(|k| template(k)).collect();
            let expected: BTreeSet<String> = KEY_TEMPLATES
                .iter()
                .filter(|key| **key != "changed")
                .map(|key| (*key).to_string())
                .collect();
            assert_eq!(encoded, expected, "{json}");
        }

        #[test]
        fn an_apply_carries_its_fields_and_the_changed_spellings() {
            let mut icons = IconTable::new();
            icons.sync(&full_spec());
            let json = apply_json(
                &full_spec(),
                &[ChromeField::Title, ChromeField::Brightness],
                &icons,
            )
            .expect("JSON fields");
            assert_eq!(
                json,
                r#"{"title":"Inbox","dark":true,"changed":["Title","Brightness"]}"#
            );
            for field in INSTALL_FIELDS {
                assert_eq!(field_name(field), format!("{field:?}"));
            }
            assert_eq!(field_name(ChromeField::Scroll), "Scroll");
            let keys = json_keys(&apply_json(&full_spec(), &INSTALL_FIELDS, &icons).unwrap());
            assert!(
                keys.iter()
                    .all(|key| names(KOTLIN_KEY_HEADER, &template(key)) || key == "changed")
            );
        }

        #[test]
        fn scroll_never_travels_as_json_and_tabs_carry_the_selection() {
            let icons = IconTable::new();
            assert_eq!(
                apply_json(&full_spec(), &[ChromeField::Scroll], &icons),
                None
            );
            assert_eq!(
                wire_fields(&[ChromeField::Tabs, ChromeField::Scroll]),
                [ChromeField::Tabs, ChromeField::Selected]
            );
            let json = apply_json(&full_spec(), &[ChromeField::Tabs], &icons).unwrap();
            assert!(json.contains(r#""selectedIndex":1"#), "{json}");
            assert!(
                json.ends_with(r#""changed":["Tabs","Selected"]}"#),
                "{json}"
            );
        }

        #[test]
        fn values_escape_and_pack_by_index() {
            let mut spec = full_spec();
            spec.top.as_mut().unwrap().title = "say \"hi\"\\\n\u{1}".into();
            spec.top.as_mut().unwrap().subtitle = None;
            spec.tabs[1] = spec.tabs[1].clone().badge("");
            let mut icons = IconTable::new();
            icons.sync(&spec);
            let json = install_json(&spec, &icons);
            for needle in [
                r#""title":"say \"hi\"\\\n\u0001""#,
                r#""subtitle":null"#,
                r#""tabCount":2"#,
                r#""tab0Title":"Home""#,
                r#""tab0Badge":"3""#,
                r#""tab0IconRev":1"#,
                r#""tab1Enabled":false"#,
                r#""tab1Badge":"""#,
                r#""tab1IconRev":0"#,
                r#""selectedIndex":1"#,
                r#""hasTop":true"#,
                r#""leading":"image""#,
                r#""actionCount":2"#,
                r#""action0HasIcon":true"#,
                r#""action0MenuCount":0"#,
                r#""action1HasIcon":false"#,
                r#""action1MenuCount":2"#,
                r#""menu1_0Title":"Share""#,
                r#""menu1_1Destructive":true"#,
                r#""menu1_1Enabled":false"#,
                r#""largeTitle":true"#,
                r#""dark":true"#,
            ] {
                assert!(json.contains(needle), "{needle} missing from {json}");
            }
            assert!(!json.contains("changed"));
            assert!(json.starts_with('{') && json.ends_with('}'));
        }

        #[test]
        fn absent_top_and_unknown_selection_encode_empty() {
            let spec = ChromeSpec {
                selected: Some(TabId::new("gone")),
                ..ChromeSpec::default()
            };
            assert_eq!(
                install_json(&spec, &IconTable::new()),
                r#"{"tabCount":0,"selectedIndex":-1,"hasTop":false,"title":"","subtitle":null,"leading":"none","actionCount":0,"largeTitle":false,"dark":false}"#
            );
            assert_eq!(leading_name(&Leading::Symbol("person".into())), "none");
            assert_eq!(leading_name(&Leading::Back), "back");
        }

        #[test]
        fn icons_are_sent_once_per_content_and_cleared_when_gone() {
            let mut icons = IconTable::new();
            let first = icons.sync(&full_spec());
            assert_eq!(
                first
                    .iter()
                    .map(|up| (up.slot, up.index, up.bytes.as_deref().map(<[u8]>::to_vec)))
                    .collect::<Vec<_>>(),
                [
                    (ICON_SLOT_TAB, 0, Some(vec![1])),
                    (ICON_SLOT_ACTION, 0, Some(vec![2])),
                    (ICON_SLOT_LEADING, 0, Some(vec![7])),
                ]
            );
            // Same content in fresh Arcs: nothing to send, revisions kept.
            assert!(icons.sync(&full_spec()).is_empty());
            assert_eq!(icons.rev(ICON_SLOT_TAB, 0), 1);
            // Changed bytes re-send at a fresh revision; a gone icon clears.
            let mut next = full_spec();
            next.tabs[0] = TabItem::new("home", "Home", TabIcon::Bytes(bytes(&[9])));
            next.top.as_mut().unwrap().leading = Leading::Back;
            let uploads = icons.sync(&next);
            assert_eq!(
                uploads,
                [
                    IconUpload {
                        slot: ICON_SLOT_LEADING,
                        index: 0,
                        bytes: None
                    },
                    IconUpload {
                        slot: ICON_SLOT_TAB,
                        index: 0,
                        bytes: Some(bytes(&[9]))
                    },
                ]
            );
            assert_eq!(icons.rev(ICON_SLOT_TAB, 0), 4);
            assert_eq!(icons.rev(ICON_SLOT_LEADING, 0), 0);
            // An Apple symbol is title-only: no bytes, no revision.
            assert_eq!(icons.rev(ICON_SLOT_TAB, 1), 0);
            icons.clear();
            assert_eq!(icons.sync(&next).len(), 2, "a cleared table re-sends all");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::host::{ChromeField, HostCall, RECORDED_CALLS_CAP};
    use super::*;
    use std::sync::Mutex as StdMutex;

    fn bytes(content: &[u8]) -> Arc<[u8]> {
        Arc::from(content.to_vec().into_boxed_slice())
    }

    fn spec() -> ChromeSpec {
        ChromeSpec {
            tabs: vec![
                TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())),
                TabItem::new("inbox", "Inbox", TabIcon::Bytes(bytes(&[1, 2, 3]))),
            ],
            selected: Some(TabId::new("home")),
            top: Some(TopBarSpec {
                title: "Home".into(),
                subtitle: None,
                leading: Leading::Back,
                actions: vec![
                    ActionItem::new("add", "Add").icon(TabIcon::AppleSymbol("plus".into())),
                    ActionItem::new("more", "More").menu(vec![
                        MenuItem::new("share", "Share"),
                        MenuItem::new("delete", "Delete").destructive(true),
                    ]),
                ],
                large_title: true,
            }),
            scroll: None,
            dark: false,
        }
    }

    fn slot() -> ChromeSlot<RecordingChromeHost> {
        ChromeSlot::new(RecordingChromeHost::new())
    }

    fn installed(slot: &mut ChromeSlot<RecordingChromeHost>) -> ChromeGeneration {
        let outcome = slot.publish(None, spec());
        slot.host_mut().take_calls();
        outcome.generation().expect("first publish installs")
    }

    #[test]
    fn the_first_publish_installs_with_a_nonzero_generation() {
        let mut slot = slot();
        let outcome = slot.publish(None, spec());
        let PublishOutcome::Installed(generation) = outcome else {
            panic!("expected Installed, got {outcome:?}");
        };
        assert_ne!(generation.get(), 0);
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Install {
                generation,
                spec: spec()
            }]
        );
    }

    #[test]
    fn a_brightness_only_publish_is_an_applied_brightness_diff() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        let mut next = spec();
        next.dark = !next.dark;
        assert_eq!(
            slot.publish(Some(generation), next.clone()),
            PublishOutcome::Applied(generation)
        );
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Apply {
                changed: vec![ChromeField::Brightness],
                spec: next,
            }]
        );
    }

    #[test]
    fn a_diff_applies_only_the_changed_fields() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        let mut next = spec();
        next.selected = Some(TabId::new("inbox"));
        next.top.as_mut().unwrap().title = "Inbox".into();
        next.scroll = Some(ScrollState {
            offset: 12.0,
            max_offset: 400.0,
            viewport: 600.0,
        });
        assert_eq!(
            slot.publish(Some(generation), next.clone()),
            PublishOutcome::Applied(generation)
        );
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Apply {
                changed: vec![
                    ChromeField::Selected,
                    ChromeField::Title,
                    ChromeField::Scroll
                ],
                spec: next.clone(),
            }]
        );
        assert_eq!(slot.spec(), Some(&next), "the new spec is retained");
    }

    #[test]
    fn every_field_is_reported_on_its_own() {
        let base = spec();
        type Edit = (ChromeField, fn(&mut ChromeSpec));
        let edits: [Edit; 9] = [
            (ChromeField::Tabs, |s| s.tabs.truncate(1)),
            (ChromeField::Selected, |s| s.selected = None),
            (ChromeField::Title, |s| {
                s.top.as_mut().unwrap().title.push('!')
            }),
            (ChromeField::Subtitle, |s| {
                s.top.as_mut().unwrap().subtitle = Some("2 new".into());
            }),
            (ChromeField::Leading, |s| {
                s.top.as_mut().unwrap().leading = Leading::Symbol("person".into());
            }),
            (ChromeField::Actions, |s| {
                s.top.as_mut().unwrap().actions.pop();
            }),
            (ChromeField::LargeTitle, |s| {
                s.top.as_mut().unwrap().large_title = false;
            }),
            (ChromeField::Scroll, |s| {
                s.scroll = Some(ScrollState::default())
            }),
            (ChromeField::Brightness, |s| s.dark = !s.dark),
        ];
        for (field, edit) in edits {
            let mut next = base.clone();
            edit(&mut next);
            let diff = ChromeDiff::between(&base, &next);
            assert_eq!(diff.changed, [field], "{field:?}");
            assert!(diff.contains(field));
        }
        // An empty bar appearing changes presence alone.
        let bare = ChromeSpec::default();
        let shown = ChromeSpec {
            top: Some(TopBarSpec::default()),
            ..ChromeSpec::default()
        };
        assert_eq!(
            ChromeDiff::between(&bare, &shown).changed,
            [ChromeField::TopBar]
        );
        assert!(ChromeDiff::between(&base, &base.clone()).is_empty());
    }

    #[test]
    fn an_identical_publish_makes_no_host_call() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        assert_eq!(
            slot.publish(Some(generation), spec()),
            PublishOutcome::Unchanged(generation)
        );
        assert_eq!(slot.host().calls().count(), 0);
    }

    #[test]
    fn icon_bytes_compare_by_content_not_identity() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        let mut rebuilt = spec();
        rebuilt.tabs[1] = TabItem::new("inbox", "Inbox", TabIcon::Bytes(bytes(&[1, 2, 3])));
        rebuilt.top.as_mut().unwrap().leading = Leading::Back;
        assert_eq!(
            slot.publish(Some(generation), rebuilt.clone()),
            PublishOutcome::Unchanged(generation),
            "a fresh Arc with the same bytes is the same spec"
        );
        let a = Leading::Image(bytes(&[9, 9]));
        assert_eq!(a, Leading::Image(bytes(&[9, 9])));
        assert_ne!(a, Leading::Image(bytes(&[9, 8])));
        rebuilt.tabs[1] = TabItem::new("inbox", "Inbox", TabIcon::Bytes(bytes(&[1, 2, 4])));
        assert_eq!(
            slot.publish(Some(generation), rebuilt),
            PublishOutcome::Applied(generation)
        );
    }

    #[test]
    fn a_second_publisher_is_refused_busy() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        assert_eq!(
            slot.publish(None, spec()),
            PublishOutcome::Refused(ChromeError::Busy)
        );
        let stale = ChromeGeneration(generation.get() + 7);
        assert_eq!(
            slot.publish(Some(stale), spec()),
            PublishOutcome::Refused(ChromeError::Busy)
        );
        assert!(!slot.withdraw(stale), "a refused scaffold cannot withdraw");
        assert_eq!(slot.generation(), Some(generation));
        assert_eq!(slot.host().calls().count(), 0);
    }

    #[test]
    fn a_withdrawn_generation_is_stale_and_only_a_fresh_publisher_installs() {
        let mut slot = slot();
        let first = installed(&mut slot);
        assert!(slot.withdraw(first));
        assert!(!slot.withdraw(first), "withdraw is once");
        assert_eq!(slot.generation(), None);
        slot.host_mut().take_calls();
        assert_eq!(slot.publish(Some(first), spec()), PublishOutcome::Stale);
        assert_eq!(
            slot.generation(),
            None,
            "a stale publisher installs nothing"
        );
        assert_eq!(slot.host().calls().count(), 0);
        let outcome = slot.publish(None, spec());
        let PublishOutcome::Installed(second) = outcome else {
            panic!("expected Installed, got {outcome:?}");
        };
        assert!(second > first, "generations increase");
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Install {
                generation: second,
                spec: spec()
            }]
        );
    }

    #[test]
    fn a_no_host_install_is_retried_on_the_next_publish() {
        let mut slot = ChromeSlot::new(RecordingChromeHost::refusing(ChromeError::NoHost));
        let outcome = slot.publish(None, spec());
        let PublishOutcome::Fallback(generation, ChromeError::NoHost) = outcome else {
            panic!("expected Fallback, got {outcome:?}");
        };
        slot.host_mut().take_calls();
        let mut next = spec();
        next.selected = None;
        assert_eq!(
            slot.publish(Some(generation), next.clone()),
            PublishOutcome::Fallback(generation, ChromeError::NoHost)
        );
        assert_eq!(
            slot.spec(),
            Some(&next),
            "fallback chrome still reads the spec"
        );
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Install {
                generation,
                spec: next.clone()
            }],
            "a NoHost install is retried under the same generation"
        );
        assert_eq!(
            slot.publish(None, spec()),
            PublishOutcome::Refused(ChromeError::Busy)
        );
        slot.host_mut().set_refusal(None);
        assert_eq!(
            slot.publish(Some(generation), spec()),
            PublishOutcome::Installed(generation)
        );
        assert_eq!(
            slot.publish(Some(generation), spec()),
            PublishOutcome::Unchanged(generation)
        );
        assert!(slot.withdraw(generation));
        assert_eq!(
            slot.host_mut().take_calls(),
            [
                HostCall::Install {
                    generation,
                    spec: spec()
                },
                HostCall::Dispose
            ]
        );
    }

    #[test]
    fn a_refused_install_pins_the_fallback_without_host_calls() {
        let refusal = ChromeError::Refused("window already re-rooted".into());
        let mut slot = ChromeSlot::new(RecordingChromeHost::refusing(refusal.clone()));
        let outcome = slot.publish(None, spec());
        let PublishOutcome::Fallback(generation, error) = outcome else {
            panic!("expected Fallback, got {outcome:?}");
        };
        assert_eq!(error, refusal);
        slot.host_mut().take_calls();
        slot.host_mut().set_refusal(None);
        let mut next = spec();
        next.selected = None;
        assert_eq!(
            slot.publish(Some(generation), next.clone()),
            PublishOutcome::Fallback(generation, refusal)
        );
        assert_eq!(slot.spec(), Some(&next));
        assert!(slot.withdraw(generation));
        assert_eq!(
            slot.host().calls().count(),
            0,
            "never retried, nothing installed, nothing disposed"
        );
    }

    #[test]
    fn a_deferred_no_host_failure_falls_back_then_retries() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        slot.host_mut()
            .fail_deferred(generation, ChromeError::NoHost);
        let mut next = spec();
        next.selected = None;
        assert_eq!(
            slot.publish(Some(generation), next.clone()),
            PublishOutcome::Fallback(generation, ChromeError::NoHost)
        );
        assert_eq!(slot.spec(), Some(&next), "the fallback reads the new spec");
        assert_eq!(slot.host().calls().count(), 0, "no apply to a dead install");
        assert_eq!(
            slot.publish(Some(generation), spec()),
            PublishOutcome::Installed(generation),
            "the next publish retries the install"
        );
        assert_eq!(
            slot.host_mut().take_calls(),
            [HostCall::Install {
                generation,
                spec: spec()
            }]
        );
    }

    #[test]
    fn a_deferred_refusal_pins_the_fallback() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        let refusal = ChromeError::Refused("a native chrome step panicked".into());
        slot.host_mut().fail_deferred(generation, refusal.clone());
        for _ in 0..2 {
            assert_eq!(
                slot.publish(Some(generation), spec()),
                PublishOutcome::Fallback(generation, refusal.clone())
            );
        }
        assert!(slot.withdraw(generation));
        assert_eq!(
            slot.host().calls().count(),
            0,
            "never retried, and the failed install is not disposed"
        );
    }

    #[test]
    fn a_failure_withdrawn_before_any_publish_is_not_disposed() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        slot.host_mut()
            .fail_deferred(generation, ChromeError::NoHost);
        assert!(slot.withdraw(generation));
        assert_eq!(slot.host().calls().count(), 0);
    }

    #[test]
    fn an_older_generations_failure_leaves_a_newer_one_alone() {
        let mut slot = slot();
        let first = installed(&mut slot);
        assert!(slot.withdraw(first));
        let second = installed(&mut slot);
        slot.host_mut().fail_deferred(first, ChromeError::NoHost);
        assert_eq!(
            slot.publish(Some(second), spec()),
            PublishOutcome::Unchanged(second)
        );
        let mut next = spec();
        next.dark = !next.dark;
        assert_eq!(
            slot.publish(Some(second), next),
            PublishOutcome::Applied(second)
        );
        assert!(slot.withdraw(second));
        assert_eq!(
            slot.host_mut().take_calls().last(),
            Some(&HostCall::Dispose),
            "the newer install is still disposed"
        );
    }

    #[test]
    fn every_chrome_event_round_trips_the_codec() {
        let spec = spec();
        for event in [
            ChromeEvent::TabSelected(TabId::new("inbox")),
            ChromeEvent::TabReselected(TabId::new("home")),
            ChromeEvent::ActionTapped(ActionId::new("add")),
            ChromeEvent::ActionTapped(ActionId::new("more")),
            ChromeEvent::MenuItemChosen(ActionId::new("more"), MenuItemId::new("delete")),
            ChromeEvent::BackPressed,
        ] {
            let (kind, detail) = encode_event(&spec, &event).expect("encodable");
            assert_eq!(
                decode_event(&spec, kind, detail),
                Some(event.clone()),
                "{event:?}"
            );
        }
        assert_eq!(
            encode_event(
                &spec,
                &ChromeEvent::MenuItemChosen(ActionId::new("more"), MenuItemId::new("delete"))
            ),
            Some((EVENT_KIND_CHROME_MENU, 1001))
        );
    }

    #[test]
    fn an_undecodable_report_is_none() {
        let spec = spec();
        for (kind, detail) in [
            (EVENT_KIND_CHROME_TAB, 2),
            (EVENT_KIND_CHROME_ACTION, 2),
            (EVENT_KIND_CHROME_MENU, 2),
            (EVENT_KIND_CHROME_MENU, 1002),
            (EVENT_KIND_CHROME_BACK, 1),
            (crate::events::EVENT_KIND_CLICK, 0),
        ] {
            assert_eq!(decode_event(&spec, kind, detail), None, "{kind}/{detail}");
        }
        assert_eq!(
            encode_event(&spec, &ChromeEvent::TabSelected(TabId::new("nope"))),
            None
        );
    }

    #[test]
    fn a_disabled_tab_or_menu_item_decodes_to_none() {
        let mut spec = spec();
        spec.tabs[1] = spec.tabs[1].clone().enabled(false);
        let actions = &mut spec.top.as_mut().unwrap().actions;
        actions[1] = ActionItem::new("more", "More").menu(vec![
            MenuItem::new("share", "Share"),
            MenuItem::new("delete", "Delete").enabled(false),
        ]);
        let tab = pack_chrome_tab(1, false).expect("packable");
        assert_eq!(decode_event(&spec, EVENT_KIND_CHROME_TAB, tab), None);
        let delete = pack_chrome_menu(1, 1).expect("packable");
        assert_eq!(decode_event(&spec, EVENT_KIND_CHROME_MENU, delete), None);
        assert_eq!(
            decode_event(&spec, EVENT_KIND_CHROME_TAB, 0),
            Some(ChromeEvent::TabSelected(TabId::new("home"))),
            "the enabled siblings still decode"
        );
        assert_eq!(
            decode_event(&spec, EVENT_KIND_CHROME_MENU, 1000),
            Some(ChromeEvent::MenuItemChosen(
                ActionId::new("more"),
                MenuItemId::new("share")
            ))
        );
    }

    #[test]
    fn a_tap_resolves_against_the_confirmed_selection() {
        let spec = spec();
        let cases = [
            (
                ChromeTap::Tab(0),
                Some(ChromeEvent::TabReselected(TabId::new("home"))),
            ),
            (
                ChromeTap::Tab(1),
                Some(ChromeEvent::TabSelected(TabId::new("inbox"))),
            ),
            (ChromeTap::Tab(2), None),
            (
                ChromeTap::Action(0),
                Some(ChromeEvent::ActionTapped(ActionId::new("add"))),
            ),
            (ChromeTap::Action(2), None),
            (
                ChromeTap::Menu(1, 1),
                Some(ChromeEvent::MenuItemChosen(
                    ActionId::new("more"),
                    MenuItemId::new("delete"),
                )),
            ),
            (ChromeTap::Menu(0, 0), None),
            (ChromeTap::Menu(1, 1000), None),
            (ChromeTap::Back, Some(ChromeEvent::BackPressed)),
        ];
        for (tap, event) in cases {
            assert_eq!(resolve_tap(&spec, tap), event, "{tap:?}");
        }
        let mut unconfirmed = spec.clone();
        unconfirmed.selected = Some(TabId::new("gone"));
        assert_eq!(
            resolve_tap(&unconfirmed, ChromeTap::Tab(0)),
            Some(ChromeEvent::TabSelected(TabId::new("home"))),
            "no confirmed tab: every tap is a selection"
        );
        unconfirmed.top.as_mut().unwrap().leading = Leading::Symbol("person".into());
        assert_eq!(resolve_tap(&unconfirmed, ChromeTap::Back), None);
        assert_eq!(resolve_tap(&ChromeSpec::default(), ChromeTap::Back), None);
    }

    #[test]
    fn a_resolved_event_is_rerouted_by_id_against_the_live_spec() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        assert!(slot.set_sink(generation, Arc::new(|_| {})));
        let shown = spec();
        let tapped = resolve_tap(&shown, ChromeTap::Action(0)).expect("resolved");
        let mut reordered = spec();
        reordered.top.as_mut().unwrap().actions.reverse();
        slot.publish(Some(generation), reordered.clone());
        let routed = slot
            .route_event(generation.get(), &tapped)
            .map(|(event, _)| event);
        assert_eq!(routed, Some(tapped.clone()), "the id survives the reorder");
        let mut gone = reordered;
        gone.top
            .as_mut()
            .unwrap()
            .actions
            .retain(|action| action.id().as_str() != "add");
        gone.tabs[1] = gone.tabs[1].clone().enabled(false);
        slot.publish(Some(generation), gone);
        assert!(slot.route_event(generation.get(), &tapped).is_none());
        let inbox = ChromeEvent::TabSelected(TabId::new("inbox"));
        assert!(
            slot.route_event(generation.get(), &inbox).is_none(),
            "disabled since"
        );
        assert!(
            slot.route_event(generation.get() + 1, &ChromeEvent::BackPressed)
                .is_none()
        );
    }

    #[test]
    fn routing_drops_a_withdrawn_generation_and_needs_a_sink() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        assert!(
            slot.route(generation.get(), EVENT_KIND_CHROME_BACK, 0)
                .is_none(),
            "no sink"
        );
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        assert!(slot.set_sink(
            generation,
            Arc::new(move |event| sink_seen.lock().unwrap().push(event))
        ));
        let (event, sink) = slot
            .route(
                generation.get(),
                EVENT_KIND_CHROME_TAB,
                pack_chrome_tab(1, false).expect("packable"),
            )
            .expect("routed");
        sink(event);
        assert_eq!(
            *seen.lock().unwrap(),
            [ChromeEvent::TabSelected(TabId::new("inbox"))]
        );
        assert!(
            slot.route(generation.get() + 1, EVENT_KIND_CHROME_BACK, 0)
                .is_none()
        );
        assert!(slot.withdraw(generation));
        assert!(
            slot.route(generation.get(), EVENT_KIND_CHROME_BACK, 0)
                .is_none()
        );
        assert!(!slot.set_sink(generation, Arc::new(|_| {})));
    }

    #[test]
    fn the_recording_log_is_bounded() {
        let mut slot = slot();
        let generation = installed(&mut slot);
        for offset in 0..(RECORDED_CALLS_CAP + 10) {
            let mut next = spec();
            next.scroll = Some(ScrollState {
                offset: offset as f64,
                ..ScrollState::default()
            });
            slot.publish(Some(generation), next);
        }
        assert_eq!(slot.host().calls().count(), RECORDED_CALLS_CAP);
    }

    #[test]
    fn the_process_wide_slot_delivers_outside_its_lock() {
        let _guard = slot_test_guard();
        let generation = publish(None, spec()).generation().expect("installed");
        assert_eq!(
            publish(None, spec()),
            PublishOutcome::Refused(ChromeError::Busy)
        );
        let republished = Arc::new(StdMutex::new(None));
        let seen = Arc::clone(&republished);
        assert!(set_sink(
            generation,
            Arc::new(move |event| {
                // A sink may publish again: deliver released the lock first.
                *seen.lock().unwrap() = Some((event, publish(Some(generation), spec())));
            })
        ));
        assert!(deliver(generation.get(), EVENT_KIND_CHROME_BACK, 0));
        assert_eq!(
            republished.lock().unwrap().take(),
            Some((
                ChromeEvent::BackPressed,
                PublishOutcome::Unchanged(generation)
            ))
        );
        assert!(deliver_event(generation.get(), &ChromeEvent::BackPressed));
        assert_eq!(
            republished.lock().unwrap().take().map(|(event, _)| event),
            Some(ChromeEvent::BackPressed)
        );
        assert!(withdraw(generation));
        assert!(!deliver(generation.get(), EVENT_KIND_CHROME_BACK, 0));
        assert!(!deliver_event(generation.get(), &ChromeEvent::BackPressed));
    }
}
