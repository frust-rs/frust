//! Native presentations: platform-owned modal UI (an alert, and on iOS/iPadOS
//! a sheet) requested imperatively from Rust and resolved to exactly one
//! terminal outcome.
//!
//! # Why not a platform-view slot
//!
//! A control is a `platform_view` slot the differ owns: it disposes a slot
//! that goes missing from the ingest stream and knows nothing about modal
//! stacking. A modal belongs to the host instead — the resumed Android
//! `Activity`, the topmost iOS view controller, the macOS key window — so a
//! presentation is a **request** ([`show_alert`], [`show_sheet`]) answered by
//! a [`Presentation`] future, never a node in the tree.
//!
//! # The contract
//!
//! - **One in flight, process-wide — across every kind.** A second request
//!   while one is live resolves [`PresentError::Busy`] immediately — no
//!   queueing, no replacing. The slot is shared by every presentation kind:
//!   an alert and a sheet can never both be live, so a [`show_sheet`] while
//!   an alert is up (or the reverse) is refused `Busy` too.
//! - **Exactly one terminal outcome.** Each accepted request is stamped with
//!   a fresh, never-zero **generation**, and resolves through a one-shot
//!   channel whose sending half is consumed by its first use: an action, a
//!   cancellation, a programmatic [`dismiss`], or the host going away
//!   ([`AlertOutcome::HostLost`], [`SheetOutcome::HostLost`]). A platform
//!   callback arriving for any other generation finds no live entry and is
//!   dropped. A sheet's detent changes are **not** outcomes: they stream
//!   through [`SheetSpec::on_detent`] while the sheet stays live.
//! - **The slot frees itself.** It is released when the outcome is sent
//!   (before the caller is woken, so the caller's continuation may present
//!   again), when an arm drops its sender without sending (resolving
//!   [`PresentError::Platform`]), or when the caller drops the
//!   [`Presentation`]. Dropping the future frees only this module's
//!   bookkeeping: the platform UI stays up until the user answers it, and
//!   that answer is discarded.
//! - **Awaitable anywhere.** [`Presentation`] is a plain [`Future`] with no
//!   executor dependency: resolution is driven by the platform's main thread,
//!   so it can be polled from `frust::spawn_local` or any other executor.
//!   Nothing ever blocks waiting for a modal result.
//!
//! # Platform arms
//!
//! `AlertHost` is the alert seam, selected by `cfg` per OS:
//!
//! - **iOS/iPadOS** — `apple_alert`: a `UIAlertController` (alert or action
//!   sheet, an iPad action sheet anchored as a popover) built over
//!   `apple_host`'s shared helpers.
//! - **macOS** — `appkit_alert`: an `NSAlert` presented as a window sheet
//!   (`beginSheetModalForWindow:completionHandler:`) on the key/main window,
//!   also built over `apple_host`'s shared helpers. macOS has no action-sheet
//!   idiom, so [`AlertStyle::ActionSheet`] presents the same sheet as
//!   [`AlertStyle::Alert`] and [`AlertSpec::anchor`] is ignored.
//! - **Android** — `android_alert`: an `android.app.AlertDialog`, over
//!   `android_host`'s shared JNI plumbing (the
//!   `dev.frust.nativewidgets.FrustNativePresenter` Kotlin object that
//!   tracks the resumed `Activity`, its one `nativeOnOutcome` callback and
//!   the live-presentation guard).
//! - **Every other target** — `unsupported`: [`PresentError::Unsupported`].
//!
//! `SheetHost` is the sheet seam: **iOS/iPadOS** — `apple_sheet`, a
//! plugin-owned `UIViewController` subclass presented as a page sheet under
//! `UISheetPresentationController` (detents, grabber, adaptive dismissal);
//! **every other target** — this module's own `UnsupportedSheet`
//! ([`PresentError::Unsupported`]; a macOS `NSPopover` or Android
//! `BottomSheetDialog` arm is follow-up work).
//!
//! `apple_host` holds what every Apple arm shares: host discovery, the
//! main-queue hop and the live-presentation guard.

mod oneshot;

#[cfg(target_os = "android")]
mod android_alert;
#[cfg(target_os = "android")]
mod android_host;
#[cfg(target_os = "macos")]
mod appkit_alert;
#[cfg(target_os = "ios")]
mod apple_alert;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple_host;
#[cfg(target_os = "ios")]
mod apple_sheet;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod unsupported;

#[cfg(target_os = "android")]
use android_alert::Host as PlatformHost;
#[cfg(target_os = "macos")]
use appkit_alert::Host as PlatformHost;
#[cfg(target_os = "ios")]
use apple_alert::Host as PlatformHost;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
use unsupported::Host as PlatformHost;

#[cfg(not(target_os = "ios"))]
use UnsupportedSheet as SheetPlatformHost;
#[cfg(target_os = "ios")]
use apple_sheet::Host as SheetPlatformHost;

pub(crate) use oneshot::Sender;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Instant;

/// The most actions one alert takes — the common ceiling of the three
/// platforms (Android's `AlertDialog` has exactly three button slots).
pub const MAX_ALERT_ACTIONS: usize = 3;

/// What an alert asks: a title, a message, up to [`MAX_ALERT_ACTIONS`]
/// actions, and how it is presented.
///
/// Validated when submitted ([`show_alert`]), before any platform API is
/// touched: at least one action and at most [`MAX_ALERT_ACTIONS`] actions,
/// every action id non-empty and unique (the id is what
/// [`AlertOutcome::Action`] reports back), at most one [`ActionRole::Cancel`]
/// action (UIKit raises on a second one), and an [`AnchorRect`] — when given
/// — finite with a non-negative size. An [`AlertStyle::ActionSheet`] on iPad
/// additionally requires `anchor`; that rule is the iOS arm's, checked when
/// it presents (only it can tell an iPad from an iPhone).
#[derive(Clone, Debug, PartialEq)]
pub struct AlertSpec {
    /// The alert's title.
    pub title: String,
    /// The alert's body text.
    pub message: String,
    /// The buttons, in presentation order.
    pub actions: Vec<AlertAction>,
    /// Whether the user may dismiss the alert without choosing an action
    /// (back key / outside tap where the platform has one) — answered as
    /// [`AlertOutcome::Cancelled`].
    pub cancelable: bool,
    /// Centered alert or action sheet.
    pub style: AlertStyle,
    /// Where an action sheet points from, in logical points of the presenting
    /// window's coordinate space (origin top-left).
    pub anchor: Option<AnchorRect>,
}

impl AlertSpec {
    /// A cancelable [`AlertStyle::Alert`] with no actions and no anchor —
    /// add buttons with [`Self::with_action`].
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            actions: Vec::new(),
            cancelable: true,
            style: AlertStyle::Alert,
            anchor: None,
        }
    }

    /// Append one action.
    #[must_use]
    pub fn with_action(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        role: ActionRole,
    ) -> Self {
        self.actions.push(AlertAction {
            id: id.into(),
            label: label.into(),
            role,
        });
        self
    }

    /// The submit-time validation described on [`AlertSpec`].
    ///
    /// # Errors
    /// [`PresentError::InvalidSpec`] naming the first rule broken.
    pub fn validate(&self) -> Result<(), PresentError> {
        if self.actions.is_empty() {
            return Err(PresentError::InvalidSpec(
                "an alert needs at least one action".to_string(),
            ));
        }
        if self.actions.len() > MAX_ALERT_ACTIONS {
            return Err(PresentError::InvalidSpec(format!(
                "an alert takes at most {MAX_ALERT_ACTIONS} actions, got {}",
                self.actions.len()
            )));
        }
        for (index, action) in self.actions.iter().enumerate() {
            if action.id.is_empty() {
                return Err(PresentError::InvalidSpec(format!(
                    "action {index} has an empty id"
                )));
            }
            if self.actions[..index].iter().any(|a| a.id == action.id) {
                return Err(PresentError::InvalidSpec(format!(
                    "duplicate action id {:?}",
                    action.id
                )));
            }
        }
        let cancels = self
            .actions
            .iter()
            .filter(|a| a.role == ActionRole::Cancel)
            .count();
        if cancels > 1 {
            return Err(PresentError::InvalidSpec(format!(
                "at most one action may take the Cancel role, got {cancels}"
            )));
        }
        if let Some(anchor) = &self.anchor
            && !anchor.is_valid()
        {
            return Err(PresentError::InvalidSpec(
                "the anchor rect must be finite with a non-negative size".to_string(),
            ));
        }
        Ok(())
    }
}

/// One alert button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertAction {
    /// Reported back verbatim as [`AlertOutcome::Action`] when chosen.
    pub id: String,
    /// The button's visible label.
    pub label: String,
    /// How the platform styles and places it.
    pub role: ActionRole,
}

/// An action's semantic role, mapped onto each platform's own button styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ActionRole {
    /// An ordinary action.
    #[default]
    Default,
    /// The action that backs out; at most one per alert.
    Cancel,
    /// An action that destroys data (red where the platform has a style).
    Destructive,
}

/// How an alert is presented.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AlertStyle {
    /// A centered modal alert.
    #[default]
    Alert,
    /// A sheet of actions rising from the bottom (iPhone), pointing at
    /// [`AlertSpec::anchor`] (iPad); platforms without the idiom present it
    /// as an ordinary alert.
    ActionSheet,
}

/// A rectangle in logical points of the presenting window's coordinate
/// space, origin top-left — where an action sheet's popover points from.
///
/// # Getting one
///
/// frust's logical pixels **are** view points on iOS (`frust-shell-ios`'s
/// touch/IME contract: logical points pass through with no scale division),
/// and the default shell's frust view fills its window, so any rect frust
/// reports in window space is already an anchor, unconverted:
///
/// - **A native control's slot** — the rect a `platform_view` slot publishes
///   each paint (`PlatformViewFrame::rect`, the absolute painted rect the
///   platform view is positioned from) is exactly where the control sits;
///   anchor a "more actions" sheet on the button that opened it.
/// - **A frust widget** — its window-space rect: an overlay surface's
///   `window_rect()`, or the origin a layout you control accumulates down to
///   the widget plus its size.
/// - **A point** — a zero-size rect at a tap location (the popover arrow
///   points at the point).
///
/// The iOS arm converts it into the presenting controller's view with
/// `convertRect:fromView:nil` (window base coordinates), so it also holds
/// when that controller's view is not full-window (a form sheet). A rect
/// outside the window is passed to UIKit as-is; UIKit clamps the popover
/// onto the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnchorRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width, non-negative.
    pub width: f64,
    /// Height, non-negative.
    pub height: f64,
}

impl AnchorRect {
    /// Every coordinate finite and the size non-negative.
    fn is_valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

/// How an alert ended — exactly one per accepted [`show_alert`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AlertOutcome {
    /// The user chose the action with this [`AlertAction::id`].
    Action(String),
    /// The user dismissed a cancelable alert without choosing an action.
    Cancelled,
    /// [`dismiss`] took the alert down.
    Dismissed,
    /// The presenting host (Activity, scene, window) went away before the
    /// alert was answered.
    HostLost,
}

/// The most actions one sheet takes — the same ceiling as an alert's
/// ([`MAX_ALERT_ACTIONS`]): a sheet's action rows are a short choice, not a
/// menu.
pub const MAX_SHEET_ACTIONS: usize = 3;

/// A sheet's content — the constrained schema a native sheet renders with
/// platform views only (decision D4): a title, a message, an optional image
/// and up to [`MAX_SHEET_ACTIONS`] action rows, laid out top to bottom in
/// that order. Arbitrary frust content is deliberately not accepted: frust
/// has a single render root, and a `platform_view` slot never mounts inside
/// a presented controller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SheetContent {
    /// A bold headline; `None` (or empty) omits the row.
    pub title: Option<String>,
    /// Body text, wrapped over as many lines as it needs; `None` (or empty)
    /// omits the row.
    pub message: Option<String>,
    /// Encoded image bytes (PNG/JPEG/HEIC — whatever the platform decodes),
    /// shown aspect-fit under the text. Bytes the platform cannot decode omit
    /// the row (logged), never fail the sheet.
    pub image: Option<Arc<[u8]>>,
    /// The action rows, in presentation order.
    pub actions: Vec<SheetAction>,
}

impl SheetContent {
    /// Empty content — add rows with the `with_*` builders. Presenting it
    /// empty is refused ([`SheetSpec::validate`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the title row.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the message row.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Set the image row from encoded bytes.
    #[must_use]
    pub fn with_image(mut self, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.image = Some(bytes.into());
        self
    }

    /// Append one action row.
    #[must_use]
    pub fn with_action(
        mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        role: ActionRole,
    ) -> Self {
        self.actions.push(SheetAction {
            id: id.into(),
            label: label.into(),
            role,
        });
        self
    }

    /// No row at all would render: no non-empty title or message, no image
    /// and no action.
    fn is_empty(&self) -> bool {
        self.title.as_deref().is_none_or(str::is_empty)
            && self.message.as_deref().is_none_or(str::is_empty)
            && self.image.is_none()
            && self.actions.is_empty()
    }
}

/// One sheet action row — a system button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetAction {
    /// Reported back verbatim as [`SheetOutcome::Action`] when tapped.
    pub id: String,
    /// The button's visible label.
    pub label: String,
    /// How the platform styles it: `Default` wears the sheet's tint,
    /// `Destructive` the system red, `Cancel` the secondary label colour. A
    /// sheet keeps every row where the spec puts it (unlike an alert, which
    /// moves its `Cancel` action).
    pub role: ActionRole,
}

/// A height a sheet rests at.
///
/// **iPad in regular width ignores detents**: UIKit presents a page sheet
/// there as a centered form sheet at a fixed size, and only an edge-attached
/// presentation (compact width, or compact height with
/// `prefersEdgeAttachedInCompactHeight`) honours them — see
/// `docs/LIMITATIONS.md`'s `native-sheet-ipad-regular-width-detents`. Never
/// assume detent parity across idioms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Detent {
    /// About half the screen height (UIKit's `mediumDetent`).
    Medium,
    /// The full-height sheet (UIKit's `largeDetent`).
    Large,
    /// A fraction `0 < f <= 1` of the largest height the sheet can take.
    /// Needs iOS 16 (`customDetentWithIdentifier:resolver:`), checked at
    /// runtime: below it the arm substitutes the nearest of [`Self::Medium`]
    /// (`f <= 0.75`) or [`Self::Large`] and logs once, and
    /// [`SheetSpec::on_detent`] then reports that substitute.
    Custom(f64),
}

impl Detent {
    /// A [`Self::Custom`] fraction must be finite with `0 < f <= 1`.
    fn is_valid(self) -> bool {
        match self {
            Self::Medium | Self::Large => true,
            Self::Custom(fraction) => fraction.is_finite() && fraction > 0.0 && fraction <= 1.0,
        }
    }

    /// The system detent a [`Self::Custom`] stands in as where custom
    /// detents do not exist (iOS 15): the nearest of [`Self::Medium`]
    /// (`f <= 0.75`, the midpoint between about-half and all) and
    /// [`Self::Large`]. Every other detent is itself.
    #[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
    pub(crate) fn system_fallback(self) -> Self {
        match self {
            Self::Custom(fraction) if fraction <= 0.75 => Self::Medium,
            Self::Custom(_) => Self::Large,
            other => other,
        }
    }
}

/// The callback [`SheetSpec::on_detent`] registers, behind an `Arc` so the
/// spec stays `Clone`. Compared by identity, printed opaquely.
#[derive(Clone)]
struct DetentListener(Arc<dyn Fn(Detent) + Send + Sync>);

impl fmt::Debug for DetentListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DetentListener(..)")
    }
}

impl PartialEq for DetentListener {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a sheet asks: its [`SheetContent`], the detents it rests at and how
/// its sheet chrome behaves.
///
/// Validated when submitted ([`show_sheet`]), before any platform API is
/// touched — see [`Self::validate`].
#[derive(Clone, Debug, PartialEq)]
pub struct SheetSpec {
    /// What the sheet shows.
    pub content: SheetContent,
    /// The heights the sheet may rest at, smallest first by convention;
    /// non-empty, no duplicates. Default `[Medium, Large]`.
    pub detents: Vec<Detent>,
    /// The detent the sheet opens at; `None` lets the platform choose (the
    /// smallest). Must be one of [`Self::detents`].
    pub selected: Option<Detent>,
    /// Show the grabber bar at the top edge (default `true`).
    pub grabber: bool,
    /// Whether scrolling to a scroll view's edge grows the sheet to its next
    /// detent instead of scrolling (UIKit's
    /// `prefersScrollingExpandsWhenScrolledToEdge`; default `true`).
    pub scrolling_expands: bool,
    /// The largest detent at which the content behind the sheet stays
    /// undimmed and interactive; `None` dims at every detent. Must be one of
    /// [`Self::detents`].
    pub largest_undimmed: Option<Detent>,
    /// The sheet's corner radius in points; `None` keeps the system radius.
    pub corner_radius: Option<f64>,
    /// Whether the user may swipe the sheet away — answered as
    /// [`SheetOutcome::Dismissed`]`(`[`DismissReason::User`]`)`. `false`
    /// leaves only an action or a programmatic dismissal (default `true`).
    pub dismissible: bool,
    /// A packed ARGB tint the `Default`-role action rows (and any other
    /// tint-following chrome) wear; `None` keeps the system tint. With the
    /// `frust-api` feature, `SheetSpec::with_theme` fills it (and
    /// [`Self::dark`]) from the active theme.
    pub tint: Option<u32>,
    /// Pin the sheet's light/dark appearance; `None` follows the system.
    pub dark: Option<bool>,
    on_detent: Option<DetentListener>,
}

impl SheetSpec {
    /// A dismissible `[Medium, Large]` sheet with a grabber, showing
    /// `content`.
    pub fn new(content: SheetContent) -> Self {
        Self {
            content,
            detents: vec![Detent::Medium, Detent::Large],
            selected: None,
            grabber: true,
            scrolling_expands: true,
            largest_undimmed: None,
            corner_radius: None,
            dismissible: true,
            tint: None,
            dark: None,
            on_detent: None,
        }
    }

    /// Replace the detents.
    #[must_use]
    pub fn with_detents(mut self, detents: impl IntoIterator<Item = Detent>) -> Self {
        self.detents = detents.into_iter().collect();
        self
    }

    /// Open at `detent` (one of [`Self::detents`]).
    #[must_use]
    pub fn with_selected(mut self, detent: Detent) -> Self {
        self.selected = Some(detent);
        self
    }

    /// Set [`Self::dismissible`].
    #[must_use]
    pub fn with_dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    /// Stream the user's detent changes into `listener` — intermediate
    /// events while the sheet stays live, never terminal outcomes. Called on
    /// the platform's main thread (frust's UI thread), once per change the
    /// **user** makes (a drag, a grabber tap); a programmatic
    /// [`SheetHandle::select_detent`] is not echoed back, following UIKit.
    #[must_use]
    pub fn on_detent(mut self, listener: impl Fn(Detent) + Send + Sync + 'static) -> Self {
        self.on_detent = Some(DetentListener(Arc::new(listener)));
        self
    }

    /// The listener [`Self::on_detent`] registered, if any.
    #[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
    pub(crate) fn detent_listener(&self) -> Option<Arc<dyn Fn(Detent) + Send + Sync>> {
        self.on_detent
            .as_ref()
            .map(|listener| Arc::clone(&listener.0))
    }

    /// The submit-time validation: the content is not empty (a non-empty
    /// title or message, an image or an action) and any image is non-empty
    /// bytes; at most [`MAX_SHEET_ACTIONS`] actions, every action id
    /// non-empty and unique; at least one detent, no duplicates, every
    /// [`Detent::Custom`] fraction finite with `0 < f <= 1`; `selected` and
    /// `largest_undimmed`, when given, among `detents`; a `corner_radius`,
    /// when given, finite and non-negative.
    ///
    /// # Errors
    /// [`PresentError::InvalidSpec`] naming the first rule broken.
    pub fn validate(&self) -> Result<(), PresentError> {
        let invalid = |message: String| Err(PresentError::InvalidSpec(message));
        if self.content.is_empty() {
            return invalid(
                "a sheet needs content: a title, a message, an image or an action".to_string(),
            );
        }
        if self.content.image.as_deref().is_some_and(<[u8]>::is_empty) {
            return invalid("the sheet's image bytes are empty".to_string());
        }
        let actions = &self.content.actions;
        if actions.len() > MAX_SHEET_ACTIONS {
            return invalid(format!(
                "a sheet takes at most {MAX_SHEET_ACTIONS} actions, got {}",
                actions.len()
            ));
        }
        for (index, action) in actions.iter().enumerate() {
            if action.id.is_empty() {
                return invalid(format!("action {index} has an empty id"));
            }
            if actions[..index].iter().any(|a| a.id == action.id) {
                return invalid(format!("duplicate action id {:?}", action.id));
            }
        }
        if self.detents.is_empty() {
            return invalid("a sheet needs at least one detent".to_string());
        }
        for (index, detent) in self.detents.iter().enumerate() {
            if !detent.is_valid() {
                return invalid(format!(
                    "detent {index} ({detent:?}): a custom fraction must be finite with \
                     0 < f <= 1"
                ));
            }
            if self.detents[..index].contains(detent) {
                return invalid(format!("duplicate detent {detent:?}"));
            }
        }
        for (name, detent) in [
            ("selected", self.selected),
            ("largest_undimmed", self.largest_undimmed),
        ] {
            if let Some(detent) = detent
                && !self.detents.contains(&detent)
            {
                return invalid(format!("{name} ({detent:?}) is not one of the detents"));
            }
        }
        if let Some(radius) = self.corner_radius
            && !(radius.is_finite() && radius >= 0.0)
        {
            return invalid("the corner radius must be finite and non-negative".to_string());
        }
        Ok(())
    }
}

/// Why a sheet went away without an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DismissReason {
    /// The user swiped it down (only a [`SheetSpec::dismissible`] sheet).
    User,
    /// [`SheetHandle::dismiss`] (or [`dismiss`]) took it down.
    Programmatic,
}

/// How a sheet ended — exactly one per accepted [`show_sheet`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SheetOutcome {
    /// The user tapped the action row with this [`SheetAction::id`]; the
    /// sheet has already finished dismissing itself.
    Action(String),
    /// The sheet went away without an action.
    Dismissed(DismissReason),
    /// The presenting host went away before the sheet was answered.
    HostLost,
}

/// Names one accepted sheet: take it down or move it between detents. Stale
/// once that sheet has resolved — both calls then do nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SheetHandle {
    generation: u64,
}

impl SheetHandle {
    /// The generic [`PresentationHandle`] for the same sheet ([`dismiss`]
    /// takes it too).
    pub fn presentation(&self) -> PresentationHandle {
        PresentationHandle {
            generation: self.generation,
        }
    }

    /// Animate the sheet to `detent` — ignored (logged) when `detent` is not
    /// one of its [`SheetSpec::detents`] or is an invalid
    /// [`Detent::Custom`]. Not echoed to [`SheetSpec::on_detent`].
    pub fn select_detent(&self, detent: Detent) {
        select_detent_on::<SheetPlatformHost>(self, detent);
    }

    /// Take the sheet down; it resolves
    /// [`SheetOutcome::Dismissed`]`(`[`DismissReason::Programmatic`]`)`
    /// exactly once.
    pub fn dismiss(&self) {
        dismiss_sheet_on::<SheetPlatformHost>(self);
    }
}

impl Presentation<SheetOutcome> {
    /// The [`SheetHandle`] for this sheet — `None` when the request was
    /// refused before anything was presented (like [`Self::handle`]).
    pub fn sheet_handle(&self) -> Option<SheetHandle> {
        self.handle.map(|handle| SheetHandle {
            generation: handle.generation,
        })
    }
}

/// Why a presentation could not be shown or did not complete.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PresentError {
    /// Another presentation is live — one at a time, process-wide. Recover
    /// by resolving it: take it down through its [`PresentationHandle`] with
    /// [`dismiss`], or drop its [`Presentation`] future — either frees the
    /// slot. A refusal logs the live presentation's generation and how long
    /// it has been held (where the target has a monotonic clock); see
    /// `docs/LIMITATIONS.md`'s `native-widgets-alert-busy-slot-unobserved-host-teardown`
    /// for the case where a live presentation outlives an unobserved host teardown.
    #[error("native presentation: another presentation is live")]
    Busy,
    /// No host to present over: no resumed Android `Activity`, no iOS
    /// window scene with a root view controller, no macOS key/main window.
    #[error("native presentation: no host to present over")]
    NoHost,
    /// This platform has no native presentation of this kind.
    #[error("native presentation: unsupported on this platform")]
    Unsupported,
    /// The request broke a submit-time rule (see [`AlertSpec`] /
    /// [`SheetSpec::validate`]).
    #[error("native presentation: invalid spec: {0}")]
    InvalidSpec(String),
    /// A platform failure not covered above (a JNI/Objective-C error, an arm
    /// that ended without an outcome).
    #[error("native presentation: platform error: {0}")]
    Platform(String),
}

/// Names one accepted presentation, for [`dismiss`]. Stale once that
/// presentation has resolved: dismissing it then does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresentationHandle {
    generation: u64,
}

/// The future a presentation request answers with — see the module doc's
/// *The contract*.
///
/// Resolves once; every further poll answers [`Poll::Pending`] rather than
/// panicking (a completion can be driven from a platform thread the polling
/// executor knows nothing about, and a spurious re-poll must not take a
/// process down). Dropping it releases the process-wide slot.
#[must_use = "a presentation's outcome is only observed by polling it; dropping it abandons the outcome"]
pub struct Presentation<T> {
    state: PresentationState<T>,
    handle: Option<PresentationHandle>,
}

/// Either an already-decided result (validation failure, `Busy`, a host's
/// synchronous refusal) or the live channel an arm resolves later — one
/// concrete type so both paths are the same future.
enum PresentationState<T> {
    Ready(Option<Result<T, PresentError>>),
    Pending(oneshot::Receiver<T>),
}

impl<T> Presentation<T> {
    fn ready(result: Result<T, PresentError>) -> Self {
        Self {
            state: PresentationState::Ready(Some(result)),
            handle: None,
        }
    }

    fn pending(receiver: oneshot::Receiver<T>, generation: u64) -> Self {
        Self {
            state: PresentationState::Pending(receiver),
            handle: Some(PresentationHandle { generation }),
        }
    }

    /// The handle [`dismiss`] takes — `None` when the request was refused
    /// before anything was presented (invalid spec, `Busy`, a synchronous
    /// host refusal).
    pub fn handle(&self) -> Option<PresentationHandle> {
        self.handle
    }

    /// Take the error a request refused before anything was presented
    /// (no [`Self::handle`]) resolves with, without polling — what an adapter
    /// that must answer synchronously (the `api` layer's signal adapter)
    /// reports. `None` for an accepted request, or once taken; a taken
    /// refusal is not delivered again by polling.
    #[cfg_attr(not(feature = "frust-api"), allow(dead_code))]
    pub(crate) fn take_refusal(&mut self) -> Option<PresentError> {
        match &mut self.state {
            PresentationState::Ready(slot) if matches!(slot, Some(Err(_))) => {
                slot.take().and_then(Result::err)
            }
            _ => None,
        }
    }
}

impl<T> fmt::Debug for Presentation<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match &self.state {
            PresentationState::Ready(Some(_)) => "ready",
            PresentationState::Ready(None) => "taken",
            PresentationState::Pending(_) => "pending",
        };
        f.debug_struct("Presentation")
            .field("state", &state)
            .field("handle", &self.handle)
            .finish()
    }
}

// `Presentation` never pin-projects: the ready value is moved out by
// `Option::take` and the receiver is itself `Unpin` (an `Arc`).
impl<T> Unpin for Presentation<T> {}

impl<T> Future for Presentation<T> {
    type Output = Result<T, PresentError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.get_mut().state {
            PresentationState::Ready(value) => match value.take() {
                Some(value) => Poll::Ready(value),
                None => Poll::Pending,
            },
            // A receiver whose value was already taken registers the waker
            // and answers `Pending`, exactly like one still waiting.
            PresentationState::Pending(receiver) => Pin::new(receiver).poll(cx),
        }
    }
}

/// The platform seam an alert arm implements (selected by `cfg`, see the
/// module doc's *Platform arms*).
pub(crate) trait AlertHost {
    /// Present `spec` for presentation `generation`, resolving `tx` exactly
    /// once — later, from the platform's own callback — or return an error
    /// synchronously without having sent anything. Never blocks for the
    /// user's answer. `spec` has already passed [`AlertSpec::validate`].
    fn show_alert(
        spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Result<(), PresentError>;

    /// Take presentation `generation` down programmatically, resolving it
    /// [`AlertOutcome::Dismissed`]. Must ignore a generation it is not
    /// presenting: the live check [`dismiss`] makes first can race a
    /// platform callback resolving that very presentation.
    fn dismiss(generation: u64);
}

/// A live presentation: its generation and when its slot was claimed — the
/// claim time backs a Busy refusal's diagnostic ([`submit`]).
struct ActiveSlot {
    generation: u64,
    claimed_at: Option<Instant>,
}

/// The live presentation, if any — the process-wide Busy slot.
static ACTIVE: Mutex<Option<ActiveSlot>> = Mutex::new(None);

/// The counter behind [`next_generation`]. Starts at `1` so a generation is
/// never `0` — a zero-initialized `jlong`/`u64` arriving from a host that
/// lost track of its own presentation must not look like a valid one.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A fresh, process-wide generation. Never `0`, including across the
/// (theoretical) wrap of [`NEXT_GENERATION`].
fn next_generation() -> u64 {
    loop {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        if generation != 0 {
            return generation;
        }
    }
}

/// Capture the current monotonic clock, or `None` on targets where no such
/// clock exists (wasm32-unknown-unknown). On native platforms, `Some(Instant::now())`;
/// on wasm32, `None` without touching [`Instant`] to avoid a platform-panic
/// where the clock is unavailable. The shim exists to let `show_alert` on web
/// resolve `Err(PresentError::Unsupported)` instead of panicking, per the
/// fail-soft contract in `frust-native-widgets` lib.rs.
#[inline]
fn claim_clock() -> Option<Instant> {
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Instant::now())
    }
}

/// Format the Busy refusal message logged when a second presentation is
/// requested while one is live. Formats the live presentation's generation
/// and the age of its slot if available (some platforms have no monotonic clock).
/// Used by [`submit`] and asserted by tests; the single source of the logged text.
fn busy_refusal_message(generation: u64, age: Option<std::time::Duration>) -> String {
    match age {
        Some(duration) => format!(
            "frust-native-widgets: presentation request refused: presentation {} has been live for {:.1}s \
             — dismiss it through its handle or drop its future to free the slot",
            generation,
            duration.as_secs_f64()
        ),
        None => format!(
            "frust-native-widgets: presentation request refused: presentation {} has been live for \
             (age unavailable on this target) — dismiss it through its handle or drop its future to free the slot",
            generation
        ),
    }
}

/// Lock [`ACTIVE`], recovering from poisoning instead of panicking — a panic
/// on either side (a polling caller, a platform callback) must not turn the
/// other side's next call into a panic too. The guarded data is a plain
/// `Option<ActiveSlot>`, coherent after any panic.
fn lock_active() -> MutexGuard<'static, Option<ActiveSlot>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Release the Busy slot if it still holds `generation` — the hook both
/// channel halves call (`oneshot`'s module doc). A no-op when a newer
/// presentation holds the slot or none does.
pub(crate) fn release_if_live(generation: u64) {
    let mut active = lock_active();
    if active
        .as_ref()
        .is_some_and(|slot| slot.generation == generation)
    {
        *active = None;
    }
}

/// The live presentation's generation and how long its slot has been held —
/// the same claim a Busy refusal logs ([`submit`]), read directly by tests.
/// `None` when the slot is free. The age is `None` on targets without a
/// monotonic clock (wasm32-unknown-unknown). Test-only: production code reads
/// the claim under the lock it already holds ([`submit`]), never through this
/// second lock acquisition.
#[cfg(test)]
pub(crate) fn live_since() -> Option<(u64, Option<std::time::Duration>)> {
    lock_active().as_ref().map(|slot| {
        (
            slot.generation,
            slot.claimed_at.map(|instant| instant.elapsed()),
        )
    })
}

/// Claim the Busy slot for a fresh generation, then hand the sending half to
/// `start`. The lock is released before `start` runs: a host that resolves
/// synchronously releases the slot through the sender, which must not
/// deadlock on a lock this function still holds.
fn submit<T>(start: impl FnOnce(Sender<T>, u64) -> Result<(), PresentError>) -> Presentation<T> {
    let generation = {
        let mut active = lock_active();
        if let Some(live) = active.as_ref() {
            let age = live.claimed_at.map(|instant| instant.elapsed());
            let message = busy_refusal_message(live.generation, age);
            log::warn!("{}", message);
            return Presentation::ready(Err(PresentError::Busy));
        }
        let generation = next_generation();
        *active = Some(ActiveSlot {
            generation,
            claimed_at: claim_clock(),
        });
        generation
    };

    let (tx, rx) = oneshot::channel(generation);
    match start(tx, generation) {
        Ok(()) => Presentation::pending(rx, generation),
        Err(err) => {
            // The host refused before presenting anything. Dropping `rx`
            // releases the slot (a no-op if the host's dropped sender
            // already did); the refusal itself is the answer.
            drop(rx);
            Presentation::ready(Err(err))
        }
    }
}

fn show_alert_on<H: AlertHost>(spec: AlertSpec) -> Presentation<AlertOutcome> {
    if let Err(err) = spec.validate() {
        return Presentation::ready(Err(err));
    }
    submit(|tx, generation| H::show_alert(spec, tx, generation))
}

fn dismiss_on<H: AlertHost>(handle: &PresentationHandle) {
    // Checked, then released, before calling into the host: the host may
    // resolve synchronously, which re-enters `release_if_live`.
    if is_live(handle.generation) {
        H::dismiss(handle.generation);
    }
}

/// Present a native alert — see the module doc's *The contract*.
///
/// The returned [`Presentation`] resolves [`AlertOutcome`] once the user (or
/// [`dismiss`], or the host going away) ends the alert, or
/// [`PresentError`]: `InvalidSpec` / `Busy` / a synchronous host refusal
/// (`NoHost`, `Unsupported`, `Platform`) on its first poll, or a later
/// `NoHost`/`Platform` from an arm that discovered it only on the platform's
/// main thread.
pub fn show_alert(spec: AlertSpec) -> Presentation<AlertOutcome> {
    show_alert_on::<PlatformHost>(spec)
}

/// Take the presentation `handle` names down; it resolves
/// [`AlertOutcome::Dismissed`] — or, for a sheet,
/// [`SheetOutcome::Dismissed`]`(`[`DismissReason::Programmatic`]`)` —
/// exactly once. A stale handle — its presentation already resolved, or
/// superseded — is ignored.
pub fn dismiss(handle: &PresentationHandle) {
    dismiss_on::<PlatformHost>(handle);
}

/// The platform seam a sheet arm implements (selected by `cfg`, see the
/// module doc's *Platform arms*). Shares the Busy slot, the generation guard
/// and the `oneshot` channel with [`AlertHost`] — only the platform half
/// differs.
pub(crate) trait SheetHost {
    /// Present `spec` for presentation `generation`, resolving `tx` exactly
    /// once — later, from the platform's own callback — or return an error
    /// synchronously without having sent anything. Never blocks for the
    /// user's answer. `spec` has already passed [`SheetSpec::validate`].
    fn show_sheet(
        spec: SheetSpec,
        tx: Sender<SheetOutcome>,
        generation: u64,
    ) -> Result<(), PresentError>;

    /// Take presentation `generation` down programmatically, resolving it
    /// [`SheetOutcome::Dismissed`]`(`[`DismissReason::Programmatic`]`)`. Must
    /// ignore a generation it is not presenting (the same race
    /// [`AlertHost::dismiss`] names).
    fn dismiss(generation: u64);

    /// Move presentation `generation` to `detent` (already validated as a
    /// [`Detent`]; membership in the spec's detents is the arm's check).
    /// Must ignore a generation it is not presenting.
    fn select_detent(generation: u64, detent: Detent);
}

/// The sheet arm of every target without a native sheet (everything but
/// iOS/iPadOS today — macOS, Android, desktop Linux/Windows, web): every
/// request resolves [`PresentError::Unsupported`] on its first poll, and
/// nothing is ever live to dismiss or move.
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) struct UnsupportedSheet;

impl SheetHost for UnsupportedSheet {
    fn show_sheet(
        _spec: SheetSpec,
        _tx: Sender<SheetOutcome>,
        _generation: u64,
    ) -> Result<(), PresentError> {
        Err(PresentError::Unsupported)
    }

    fn dismiss(_generation: u64) {}

    fn select_detent(_generation: u64, _detent: Detent) {}
}

fn show_sheet_on<H: SheetHost>(spec: SheetSpec) -> Presentation<SheetOutcome> {
    if let Err(err) = spec.validate() {
        return Presentation::ready(Err(err));
    }
    submit(|tx, generation| H::show_sheet(spec, tx, generation))
}

/// Whether `generation` holds the Busy slot — the stale-handle filter
/// [`dismiss_on`] applies, shared by the sheet handle's calls. Released
/// before the caller reaches the host, which may resolve synchronously.
fn is_live(generation: u64) -> bool {
    lock_active()
        .as_ref()
        .is_some_and(|slot| slot.generation == generation)
}

fn dismiss_sheet_on<H: SheetHost>(handle: &SheetHandle) {
    if is_live(handle.generation) {
        H::dismiss(handle.generation);
    }
}

fn select_detent_on<H: SheetHost>(handle: &SheetHandle, detent: Detent) {
    if !detent.is_valid() {
        log::warn!(
            "frust-native-widgets: select_detent({detent:?}) ignored: a custom fraction must be \
             finite with 0 < f <= 1"
        );
        return;
    }
    if is_live(handle.generation) {
        H::select_detent(handle.generation, detent);
    }
}

/// Present a native sheet — see the module doc's *The contract*.
///
/// The returned [`Presentation`] resolves [`SheetOutcome`] once the user
/// (an action row, a swipe-down), [`SheetHandle::dismiss`] or the host going
/// away ends the sheet, or [`PresentError`]: `InvalidSpec` / `Busy` (another
/// presentation of any kind is live) / `Unsupported` (every platform but
/// iOS/iPadOS) on its first poll, or a later `NoHost`/`Platform` from the
/// arm. [`Presentation::sheet_handle`] names it for
/// [`SheetHandle::select_detent`] and [`SheetHandle::dismiss`].
///
/// On iPad in regular width the system shows a centered form sheet and
/// ignores the detents (see [`Detent`]).
pub fn show_sheet(spec: SheetSpec) -> Presentation<SheetOutcome> {
    show_sheet_on::<SheetPlatformHost>(spec)
}

/// The integer outcome codes the Android presenter reports through its one
/// `nativeOnOutcome(generation, code, actionIndex)` callback — mirrored
/// verbatim by `FrustNativePresenter.kt`'s `OUTCOME_*` constants and pinned
/// against drift by this module's tests. Consumed by the Android alert arm
/// (`android_alert`/`android_host`); on every other target this module's own
/// tests are the only reader.
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) mod wire {
    use super::{AlertOutcome, PresentError};

    /// The user chose the action at `actionIndex` (spec order).
    pub(crate) const OUTCOME_ACTION: i32 = 0;
    /// The user cancelled (back key, outside tap).
    pub(crate) const OUTCOME_CANCELLED: i32 = 1;
    /// Programmatic dismissal.
    pub(crate) const OUTCOME_DISMISSED: i32 = 2;
    /// The hosting Activity was destroyed first.
    pub(crate) const OUTCOME_HOST_LOST: i32 = 3;
    /// No resumed Activity when the show actually ran.
    pub(crate) const OUTCOME_NO_HOST: i32 = 4;
    /// The show threw.
    pub(crate) const OUTCOME_FAILED: i32 = 5;

    /// Map one `nativeOnOutcome` report onto an alert's result, reading the
    /// chosen action's id out of `action_ids` (the spec's ids, in order).
    pub(crate) fn alert_outcome(
        code: i32,
        action_index: i32,
        action_ids: &[String],
    ) -> Result<AlertOutcome, PresentError> {
        match code {
            OUTCOME_ACTION => usize::try_from(action_index)
                .ok()
                .and_then(|index| action_ids.get(index))
                .map(|id| AlertOutcome::Action(id.clone()))
                .ok_or_else(|| {
                    PresentError::Platform(format!(
                        "action index {action_index} out of range for {} actions",
                        action_ids.len()
                    ))
                }),
            OUTCOME_CANCELLED => Ok(AlertOutcome::Cancelled),
            OUTCOME_DISMISSED => Ok(AlertOutcome::Dismissed),
            OUTCOME_HOST_LOST => Ok(AlertOutcome::HostLost),
            OUTCOME_NO_HOST => Err(PresentError::NoHost),
            OUTCOME_FAILED => Err(PresentError::Platform(
                "the presenter failed to show the alert".to_string(),
            )),
            other => Err(PresentError::Platform(format!(
                "unknown presenter outcome code {other}"
            ))),
        }
    }
}

/// Test seams shared with other modules' host tests (the `api` layer's
/// adapter tests): a presentation whose sender the test holds, and the
/// process-wide Busy slot held for the duration of a closure. Every test in
/// this crate that touches [`ACTIVE`] serializes on
/// [`serialize`](test_support::serialize).
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    use super::{
        ActiveSlot, Presentation, Sender, claim_clock, lock_active, next_generation, oneshot,
    };

    /// Serializes every test that touches [`super::ACTIVE`] — one
    /// process-global slot, and `#[test]`s run on parallel threads.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Hold [`TEST_LOCK`], recovering from a test that panicked holding it.
    pub(crate) fn serialize() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// An accepted-looking presentation and the sender that resolves it,
    /// under a generation nothing is ever live under — so neither half
    /// touches the Busy slot.
    #[cfg_attr(not(feature = "frust-api"), allow(dead_code))]
    pub(crate) fn pending_pair<T>() -> (Sender<T>, Presentation<T>) {
        let (tx, rx) = oneshot::channel(u64::MAX);
        (tx, Presentation::pending(rx, u64::MAX))
    }

    /// Run `f` with the Busy slot held by some other presentation, freeing
    /// it afterwards; serialized on [`TEST_LOCK`].
    #[cfg_attr(not(feature = "frust-api"), allow(dead_code))]
    pub(crate) fn with_slot_held<R>(f: impl FnOnce() -> R) -> R {
        let _guard = serialize();
        *lock_active() = Some(ActiveSlot {
            generation: next_generation(),
            claimed_at: claim_clock(),
        });
        let result = f();
        *lock_active() = None;
        result
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::atomic::Ordering as AtomicOrdering;
    use std::task::Waker;

    use super::oneshot::tests::counting_waker;
    use super::*;

    /// A scripted stand-in host: what `show_alert` should do, the one live
    /// sender it holds, and a record of every call.
    #[derive(Default)]
    struct FakeState {
        refuse: Option<PresentError>,
        live: Option<(u64, Sender<AlertOutcome>)>,
        shown: Vec<AlertSpec>,
        dismissed: Vec<u64>,
    }

    thread_local! {
        static FAKE: RefCell<FakeState> = RefCell::new(FakeState::default());
    }

    struct FakeHost;

    impl AlertHost for FakeHost {
        fn show_alert(
            spec: AlertSpec,
            tx: Sender<AlertOutcome>,
            generation: u64,
        ) -> Result<(), PresentError> {
            FAKE.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.shown.push(spec);
                if let Some(err) = fake.refuse.clone() {
                    return Err(err);
                }
                fake.live = Some((generation, tx));
                Ok(())
            })
        }

        fn dismiss(generation: u64) {
            let taken = FAKE.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.dismissed.push(generation);
                match &fake.live {
                    Some((live, _)) if *live == generation => fake.live.take(),
                    _ => None,
                }
            });
            // Resolved outside the borrow, like a real arm resolving outside
            // its own lock.
            if let Some((_, tx)) = taken {
                tx.send(Ok(AlertOutcome::Dismissed));
            }
        }
    }

    /// The platform delivering `outcome` for whatever is live on the fake —
    /// `false` when nothing is (a duplicate or stale callback).
    fn platform_resolves(outcome: Result<AlertOutcome, PresentError>) -> bool {
        match FAKE.with(|fake| fake.borrow_mut().live.take()) {
            Some((_, tx)) => {
                tx.send(outcome);
                true
            }
            None => false,
        }
    }

    /// Run `f` holding `test_support`'s lock, with [`ACTIVE`] and the fake
    /// reset before and after.
    fn isolated<R>(f: impl FnOnce() -> R) -> R {
        let _guard = test_support::serialize();
        let reset = || {
            *lock_active() = None;
            FAKE.with(|fake| *fake.borrow_mut() = FakeState::default());
        };
        reset();
        let result = f();
        reset();
        result
    }

    fn poll_with<T>(
        presentation: &mut Presentation<T>,
        waker: &Waker,
    ) -> Poll<Result<T, PresentError>> {
        let mut cx = Context::from_waker(waker);
        Pin::new(presentation).poll(&mut cx)
    }

    fn poll_once<T>(presentation: &mut Presentation<T>) -> Poll<Result<T, PresentError>> {
        poll_with(presentation, &counting_waker().0)
    }

    fn two_button_spec() -> AlertSpec {
        AlertSpec::new("Delete draft?", "This cannot be undone.")
            .with_action("keep", "Keep", ActionRole::Cancel)
            .with_action("delete", "Delete", ActionRole::Destructive)
    }

    #[test]
    fn the_outcome_is_delivered_exactly_once() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            assert!(presentation.handle().is_some());

            let (waker, woken) = counting_waker();
            assert_eq!(poll_with(&mut presentation, &waker), Poll::Pending);

            assert!(platform_resolves(Ok(AlertOutcome::Action("delete".into()))));
            assert_eq!(woken.load(AtomicOrdering::SeqCst), 1);
            // The sender was consumed by that first delivery: a duplicate
            // platform callback has nothing left to resolve.
            assert!(!platform_resolves(Ok(AlertOutcome::Cancelled)));

            assert_eq!(
                poll_with(&mut presentation, &waker),
                Poll::Ready(Ok(AlertOutcome::Action("delete".into())))
            );
            assert_eq!(poll_with(&mut presentation, &waker), Poll::Pending);
            assert_eq!(woken.load(AtomicOrdering::SeqCst), 1);
        });
    }

    #[test]
    fn a_second_show_while_one_is_live_is_busy() {
        isolated(|| {
            let mut first = show_alert_on::<FakeHost>(two_button_spec());
            let mut second = show_alert_on::<FakeHost>(two_button_spec());

            assert_eq!(second.handle(), None);
            assert_eq!(poll_once(&mut second), Poll::Ready(Err(PresentError::Busy)));
            assert_eq!(
                FAKE.with(|fake| fake.borrow().shown.len()),
                1,
                "a Busy request must never reach the host"
            );
            // The refused request did not disturb the live one.
            assert_eq!(poll_once(&mut first), Poll::Pending);
            assert!(platform_resolves(Ok(AlertOutcome::Cancelled)));
            assert_eq!(
                poll_once(&mut first),
                Poll::Ready(Ok(AlertOutcome::Cancelled))
            );
        });
    }

    #[test]
    fn a_busy_refusal_reports_the_live_generation() {
        isolated(|| {
            let first = show_alert_on::<FakeHost>(two_button_spec());
            let first_generation = first.handle().expect("accepted").generation;

            let mut second = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(poll_once(&mut second), Poll::Ready(Err(PresentError::Busy)));

            let (generation, age) = live_since().expect("the first presentation is still live");
            assert_eq!(generation, first_generation);
            // Just claimed, well under any real hang (age is available on native targets).
            if let Some(duration) = age {
                assert!(duration < std::time::Duration::from_secs(5));
            }

            drop(first);
        });
    }

    #[test]
    fn a_busy_refusal_message_is_formatted_with_and_without_age() {
        // Test with age (Some case).
        let age_some = Some(std::time::Duration::from_secs_f64(1.5));
        let msg_with_age = busy_refusal_message(42, age_some);
        assert!(msg_with_age.contains("42"), "message includes generation");
        assert!(msg_with_age.contains("1.5"), "message includes age");
        assert!(
            msg_with_age.contains("has been live for"),
            "message includes time phrase"
        );
        assert!(
            !msg_with_age.contains("unavailable"),
            "message does not say unavailable"
        );

        // Test without age (None case).
        let msg_no_age = busy_refusal_message(43, None);
        assert!(msg_no_age.contains("43"), "message includes generation");
        assert!(
            msg_no_age.contains("age unavailable on this target"),
            "message explains no age"
        );
        assert!(
            msg_no_age.contains("has been live for"),
            "message includes time phrase"
        );
    }

    #[test]
    fn the_slot_is_free_again_once_the_outcome_is_sent() {
        isolated(|| {
            let first = show_alert_on::<FakeHost>(two_button_spec());
            assert!(platform_resolves(Ok(AlertOutcome::Cancelled)));
            // `first` is still held (not even polled): the send alone freed
            // the slot, so a continuation may present again at once.
            let mut next = show_alert_on::<FakeHost>(two_button_spec());
            assert!(next.handle().is_some());
            assert_eq!(poll_once(&mut next), Poll::Pending);
            drop(first);
        });
    }

    #[test]
    fn dismiss_resolves_dismissed_once() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            let handle = presentation.handle().expect("accepted");

            dismiss_on::<FakeHost>(&handle);
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(AlertOutcome::Dismissed))
            );

            // Now stale: filtered before it reaches the host at all.
            dismiss_on::<FakeHost>(&handle);
            assert_eq!(
                FAKE.with(|fake| fake.borrow().dismissed.clone()),
                vec![handle.generation]
            );
            assert_eq!(poll_once(&mut presentation), Poll::Pending);
        });
    }

    #[test]
    fn a_late_dismiss_for_an_old_generation_is_ignored() {
        isolated(|| {
            let old = show_alert_on::<FakeHost>(two_button_spec());
            let old_handle = old.handle().expect("accepted");
            assert!(platform_resolves(Ok(AlertOutcome::Action("keep".into()))));
            drop(old);

            let mut current = show_alert_on::<FakeHost>(two_button_spec());
            let current_handle = current.handle().expect("accepted");
            assert_ne!(old_handle, current_handle);

            dismiss_on::<FakeHost>(&old_handle);
            assert!(FAKE.with(|fake| fake.borrow().dismissed.is_empty()));
            assert_eq!(poll_once(&mut current), Poll::Pending);

            // Even a host asked directly with the old generation (the race
            // the trait doc names) leaves the live presentation alone.
            FakeHost::dismiss(old_handle.generation);
            assert_eq!(poll_once(&mut current), Poll::Pending);

            dismiss_on::<FakeHost>(&current_handle);
            assert_eq!(
                poll_once(&mut current),
                Poll::Ready(Ok(AlertOutcome::Dismissed))
            );
        });
    }

    #[test]
    fn host_lost_resolves_and_frees_the_slot() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            let handle = presentation.handle().expect("accepted");

            assert!(platform_resolves(Ok(AlertOutcome::HostLost)));
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(AlertOutcome::HostLost))
            );
            assert!(live_since().is_none());

            // A dismiss arriving after the host is gone is stale.
            dismiss_on::<FakeHost>(&handle);
            assert!(FAKE.with(|fake| fake.borrow().dismissed.is_empty()));
        });
    }

    #[test]
    fn dropping_the_presentation_releases_the_guard() {
        isolated(|| {
            let abandoned = show_alert_on::<FakeHost>(two_button_spec());
            drop(abandoned);
            assert!(live_since().is_none());

            // The abandoned alert is still up on the platform; its eventual
            // answer arrives after a newer presentation took the slot.
            let stale_sender = FAKE.with(|fake| fake.borrow_mut().live.take());
            let mut current = show_alert_on::<FakeHost>(two_button_spec());
            let (_, stale_tx) = stale_sender.expect("the fake held the first sender");
            assert!(!stale_tx.send(Ok(AlertOutcome::Cancelled)), "discarded");

            assert_eq!(poll_once(&mut current), Poll::Pending);
            assert_eq!(
                live_since().map(|(generation, _)| generation),
                current.handle().map(|h| h.generation)
            );
        });
    }

    #[test]
    fn a_host_refusal_resolves_the_error_and_frees_the_slot() {
        isolated(|| {
            FAKE.with(|fake| fake.borrow_mut().refuse = Some(PresentError::NoHost));
            let mut refused = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(refused.handle(), None);
            assert_eq!(
                poll_once(&mut refused),
                Poll::Ready(Err(PresentError::NoHost))
            );
            assert!(live_since().is_none());
        });
    }

    #[test]
    fn an_arm_dropping_its_sender_resolves_platform_and_frees_the_slot() {
        isolated(|| {
            let mut presentation = show_alert_on::<FakeHost>(two_button_spec());
            drop(FAKE.with(|fake| fake.borrow_mut().live.take()));
            assert!(live_since().is_none());
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Err(PresentError::Platform(
                    oneshot::DROPPED_WITHOUT_OUTCOME.to_string()
                )))
            );
        });
    }

    #[test]
    fn an_invalid_spec_is_refused_before_claiming_the_slot() {
        isolated(|| {
            let zero_actions = AlertSpec::new("t", "m");
            let four = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Default)
                .with_action("b", "B", ActionRole::Default)
                .with_action("c", "C", ActionRole::Default)
                .with_action("d", "D", ActionRole::Default);
            let duplicate = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Default)
                .with_action("a", "Again", ActionRole::Default);
            let empty_id = AlertSpec::new("t", "m").with_action("", "A", ActionRole::Default);
            let two_cancels = AlertSpec::new("t", "m")
                .with_action("a", "A", ActionRole::Cancel)
                .with_action("b", "B", ActionRole::Cancel);
            // One action so this fixture pins the anchor rule specifically,
            // not the (now earlier-checked) zero-action rule above.
            let mut bad_anchor =
                AlertSpec::new("t", "m").with_action("a", "A", ActionRole::Default);
            bad_anchor.style = AlertStyle::ActionSheet;
            bad_anchor.anchor = Some(AnchorRect {
                x: f64::NAN,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            });
            let mut negative_anchor = bad_anchor.clone();
            negative_anchor.anchor = Some(AnchorRect {
                x: 0.0,
                y: 0.0,
                width: -1.0,
                height: 10.0,
            });

            for spec in [
                zero_actions,
                four,
                duplicate,
                empty_id,
                two_cancels,
                bad_anchor,
                negative_anchor,
            ] {
                let mut presentation = show_alert_on::<FakeHost>(spec.clone());
                assert!(
                    matches!(
                        poll_once(&mut presentation),
                        Poll::Ready(Err(PresentError::InvalidSpec(_)))
                    ),
                    "{spec:?} should be refused"
                );
                assert!(live_since().is_none());
            }
            assert!(FAKE.with(|fake| fake.borrow().shown.is_empty()));
        });
    }

    #[test]
    fn a_refusal_is_taken_once_and_an_accepted_request_has_none() {
        isolated(|| {
            let mut accepted = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(accepted.take_refusal(), None);
            assert_eq!(poll_once(&mut accepted), Poll::Pending);

            let mut busy = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(busy.take_refusal(), Some(PresentError::Busy));
            assert_eq!(busy.take_refusal(), None);
            assert_eq!(poll_once(&mut busy), Poll::Pending);
        });
    }

    #[test]
    fn a_valid_spec_passes() {
        let mut sheet = two_button_spec().with_action("share", "Share", ActionRole::Default);
        sheet.style = AlertStyle::ActionSheet;
        sheet.anchor = Some(AnchorRect {
            x: 10.0,
            y: 20.0,
            width: 0.0,
            height: 0.0,
        });
        assert_eq!(sheet.validate(), Ok(()));
        assert_eq!(
            AlertSpec::new("", "")
                .with_action("a", "A", ActionRole::Default)
                .validate(),
            Ok(())
        );
    }

    #[test]
    fn a_zero_action_spec_is_rejected() {
        assert_eq!(
            AlertSpec::new("t", "m").validate(),
            Err(PresentError::InvalidSpec(
                "an alert needs at least one action".to_string()
            ))
        );
    }

    #[test]
    fn generations_are_never_zero_and_always_fresh() {
        let a = next_generation();
        let b = next_generation();
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        assert_ne!(a, b);
    }

    #[test]
    fn the_wire_table_maps_every_code() {
        let ids = ["keep".to_string(), "delete".to_string()];
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_ACTION, 1, &ids),
            Ok(AlertOutcome::Action("delete".into()))
        );
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_ACTION, 2, &ids),
            Err(PresentError::Platform(_))
        ));
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_ACTION, -1, &ids),
            Err(PresentError::Platform(_))
        ));
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_CANCELLED, -1, &ids),
            Ok(AlertOutcome::Cancelled)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_DISMISSED, -1, &ids),
            Ok(AlertOutcome::Dismissed)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_HOST_LOST, -1, &ids),
            Ok(AlertOutcome::HostLost)
        );
        assert_eq!(
            wire::alert_outcome(wire::OUTCOME_NO_HOST, -1, &ids),
            Err(PresentError::NoHost)
        );
        assert!(matches!(
            wire::alert_outcome(wire::OUTCOME_FAILED, -1, &ids),
            Err(PresentError::Platform(_))
        ));
        assert!(matches!(
            wire::alert_outcome(99, -1, &ids),
            Err(PresentError::Platform(_))
        ));
    }

    // --- Sheets ----------------------------------------------------------

    /// The sheet counterpart of [`FakeState`]: one live sender, and a record
    /// of every show / dismiss / detent move that reached the host.
    #[derive(Default)]
    struct FakeSheetState {
        live: Option<(u64, Sender<SheetOutcome>)>,
        shown: usize,
        dismissed: Vec<u64>,
        moved: Vec<(u64, Detent)>,
    }

    thread_local! {
        static FAKE_SHEET: RefCell<FakeSheetState> = RefCell::new(FakeSheetState::default());
    }

    struct FakeSheetHost;

    impl SheetHost for FakeSheetHost {
        fn show_sheet(
            _spec: SheetSpec,
            tx: Sender<SheetOutcome>,
            generation: u64,
        ) -> Result<(), PresentError> {
            FAKE_SHEET.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.shown += 1;
                fake.live = Some((generation, tx));
            });
            Ok(())
        }

        fn dismiss(generation: u64) {
            let taken = FAKE_SHEET.with(|fake| {
                let mut fake = fake.borrow_mut();
                fake.dismissed.push(generation);
                match &fake.live {
                    Some((live, _)) if *live == generation => fake.live.take(),
                    _ => None,
                }
            });
            if let Some((_, tx)) = taken {
                tx.send(Ok(SheetOutcome::Dismissed(DismissReason::Programmatic)));
            }
        }

        fn select_detent(generation: u64, detent: Detent) {
            FAKE_SHEET.with(|fake| fake.borrow_mut().moved.push((generation, detent)));
        }
    }

    /// The platform delivering `outcome` for whatever sheet is live — `false`
    /// when none is.
    fn sheet_resolves(outcome: Result<SheetOutcome, PresentError>) -> bool {
        match FAKE_SHEET.with(|fake| fake.borrow_mut().live.take()) {
            Some((_, tx)) => {
                tx.send(outcome);
                true
            }
            None => false,
        }
    }

    /// [`isolated`] plus the sheet fake's reset.
    fn isolated_sheet<R>(f: impl FnOnce() -> R) -> R {
        isolated(|| {
            let reset = || FAKE_SHEET.with(|fake| *fake.borrow_mut() = FakeSheetState::default());
            reset();
            let result = f();
            reset();
            result
        })
    }

    fn sheet_spec() -> SheetSpec {
        SheetSpec::new(
            SheetContent::new()
                .with_title("Share draft")
                .with_message("Pick where it goes.")
                .with_action("copy", "Copy link", ActionRole::Default)
                .with_action("delete", "Delete", ActionRole::Destructive),
        )
    }

    fn assert_invalid_sheet(spec: &SheetSpec) {
        assert!(
            matches!(spec.validate(), Err(PresentError::InvalidSpec(_))),
            "{spec:?} should be refused"
        );
    }

    #[test]
    fn a_valid_sheet_spec_passes() {
        assert_eq!(sheet_spec().validate(), Ok(()));
        // Any single row is content; a full-height custom detent is in range.
        for content in [
            SheetContent::new().with_title("t"),
            SheetContent::new().with_message("m"),
            SheetContent::new().with_image(vec![0x89, b'P', b'N', b'G']),
            SheetContent::new().with_action("ok", "OK", ActionRole::Default),
        ] {
            let spec = SheetSpec::new(content)
                .with_detents([Detent::Custom(0.25), Detent::Medium, Detent::Custom(1.0)])
                .with_selected(Detent::Custom(0.25));
            assert_eq!(spec.validate(), Ok(()), "{spec:?}");
        }
    }

    #[test]
    fn empty_sheet_content_is_refused() {
        assert_invalid_sheet(&SheetSpec::new(SheetContent::new()));
        // Empty strings are no content either.
        assert_invalid_sheet(&SheetSpec::new(
            SheetContent::new().with_title("").with_message(""),
        ));
        // An image row needs bytes.
        assert_invalid_sheet(&SheetSpec::new(
            SheetContent::new()
                .with_title("t")
                .with_image(Vec::<u8>::new()),
        ));
    }

    #[test]
    fn more_than_three_sheet_actions_are_refused() {
        let content = (0..=MAX_SHEET_ACTIONS).fold(SheetContent::new(), |content, i| {
            content.with_action(format!("a{i}"), "A", ActionRole::Default)
        });
        assert_eq!(content.actions.len(), MAX_SHEET_ACTIONS + 1);
        assert_invalid_sheet(&SheetSpec::new(content));
    }

    #[test]
    fn sheet_action_ids_must_be_non_empty_and_unique() {
        assert_invalid_sheet(&SheetSpec::new(SheetContent::new().with_action(
            "",
            "A",
            ActionRole::Default,
        )));
        assert_invalid_sheet(&SheetSpec::new(
            SheetContent::new()
                .with_action("a", "A", ActionRole::Default)
                .with_action("a", "Again", ActionRole::Cancel),
        ));
    }

    #[test]
    fn a_custom_fraction_must_lie_in_zero_exclusive_to_one_inclusive() {
        for fraction in [0.0, -0.25, 1.000_001, 2.0, f64::NAN, f64::INFINITY] {
            assert_invalid_sheet(&sheet_spec().with_detents([Detent::Custom(fraction)]));
        }
        for fraction in [f64::MIN_POSITIVE, 0.5, 1.0] {
            assert_eq!(
                sheet_spec()
                    .with_detents([Detent::Custom(fraction)])
                    .validate(),
                Ok(())
            );
        }
    }

    #[test]
    fn a_custom_detent_falls_back_to_the_nearest_system_detent() {
        assert_eq!(Detent::Custom(0.1).system_fallback(), Detent::Medium);
        assert_eq!(Detent::Custom(0.75).system_fallback(), Detent::Medium);
        assert_eq!(Detent::Custom(0.76).system_fallback(), Detent::Large);
        assert_eq!(Detent::Custom(1.0).system_fallback(), Detent::Large);
        assert_eq!(Detent::Medium.system_fallback(), Detent::Medium);
        assert_eq!(Detent::Large.system_fallback(), Detent::Large);
    }

    #[test]
    fn detent_rules_are_enforced() {
        assert_invalid_sheet(&sheet_spec().with_detents([]));
        assert_invalid_sheet(&sheet_spec().with_detents([Detent::Medium, Detent::Medium]));
        assert_invalid_sheet(
            &sheet_spec()
                .with_detents([Detent::Medium])
                .with_selected(Detent::Large),
        );
        let mut undimmed = sheet_spec().with_detents([Detent::Large]);
        undimmed.largest_undimmed = Some(Detent::Medium);
        assert_invalid_sheet(&undimmed);
        let mut radius = sheet_spec();
        radius.corner_radius = Some(-1.0);
        assert_invalid_sheet(&radius);
        radius.corner_radius = Some(f64::NAN);
        assert_invalid_sheet(&radius);
        radius.corner_radius = Some(0.0);
        assert_eq!(radius.validate(), Ok(()));
    }

    #[test]
    fn an_invalid_sheet_never_reaches_the_host_or_the_slot() {
        isolated_sheet(|| {
            let mut refused = show_sheet_on::<FakeSheetHost>(SheetSpec::new(SheetContent::new()));
            assert_eq!(refused.sheet_handle(), None);
            assert!(matches!(
                poll_once(&mut refused),
                Poll::Ready(Err(PresentError::InvalidSpec(_)))
            ));
            assert!(live_since().is_none());
            assert_eq!(FAKE_SHEET.with(|fake| fake.borrow().shown), 0);
        });
    }

    #[test]
    fn the_unsupported_sheet_arm_refuses_and_frees_the_slot() {
        isolated_sheet(|| {
            let mut refused = show_sheet_on::<UnsupportedSheet>(sheet_spec());
            assert_eq!(refused.sheet_handle(), None);
            assert_eq!(
                poll_once(&mut refused),
                Poll::Ready(Err(PresentError::Unsupported))
            );
            assert!(live_since().is_none());
            // Nothing is ever live on it, so its dismiss/move are no-ops.
            UnsupportedSheet::dismiss(u64::MAX);
            UnsupportedSheet::select_detent(u64::MAX, Detent::Large);
        });
    }

    /// The public entry point on this (non-iOS) host resolves through the
    /// unsupported arm.
    #[cfg(not(target_os = "ios"))]
    #[test]
    fn show_sheet_is_unsupported_off_ios() {
        isolated_sheet(|| {
            let mut presentation = show_sheet(sheet_spec());
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Err(PresentError::Unsupported))
            );
            assert!(live_since().is_none());
        });
    }

    #[test]
    fn a_sheet_action_resolves_exactly_once() {
        isolated_sheet(|| {
            let mut presentation = show_sheet_on::<FakeSheetHost>(sheet_spec());
            let handle = presentation.sheet_handle().expect("accepted");
            assert_eq!(Some(handle.presentation()), presentation.handle());
            assert_eq!(poll_once(&mut presentation), Poll::Pending);

            assert!(sheet_resolves(Ok(SheetOutcome::Action("copy".into()))));
            // A late swipe-down callback has nothing left to resolve.
            assert!(!sheet_resolves(Ok(SheetOutcome::Dismissed(
                DismissReason::User
            ))));
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(SheetOutcome::Action("copy".into())))
            );
            assert_eq!(poll_once(&mut presentation), Poll::Pending);
            assert!(live_since().is_none());
        });
    }

    #[test]
    fn a_user_swipe_resolves_dismissed_user() {
        isolated_sheet(|| {
            let mut presentation = show_sheet_on::<FakeSheetHost>(sheet_spec());
            assert!(sheet_resolves(Ok(SheetOutcome::Dismissed(
                DismissReason::User
            ))));
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(SheetOutcome::Dismissed(DismissReason::User)))
            );
        });
    }

    #[test]
    fn the_sheet_handle_dismisses_once_and_goes_stale() {
        isolated_sheet(|| {
            let mut presentation = show_sheet_on::<FakeSheetHost>(sheet_spec());
            let handle = presentation.sheet_handle().expect("accepted");

            dismiss_sheet_on::<FakeSheetHost>(&handle);
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(SheetOutcome::Dismissed(DismissReason::Programmatic)))
            );
            // Stale: filtered before the host, for both calls.
            dismiss_sheet_on::<FakeSheetHost>(&handle);
            select_detent_on::<FakeSheetHost>(&handle, Detent::Large);
            FAKE_SHEET.with(|fake| {
                let fake = fake.borrow();
                assert_eq!(fake.dismissed, vec![handle.generation]);
                assert!(fake.moved.is_empty());
            });
        });
    }

    #[test]
    fn select_detent_reaches_the_host_only_while_live_and_valid() {
        isolated_sheet(|| {
            let mut presentation = show_sheet_on::<FakeSheetHost>(sheet_spec());
            let handle = presentation.sheet_handle().expect("accepted");

            select_detent_on::<FakeSheetHost>(&handle, Detent::Large);
            select_detent_on::<FakeSheetHost>(&handle, Detent::Custom(0.0));
            select_detent_on::<FakeSheetHost>(&handle, Detent::Custom(f64::NAN));
            assert_eq!(
                FAKE_SHEET.with(|fake| fake.borrow().moved.clone()),
                vec![(handle.generation, Detent::Large)]
            );
            // A detent move is not an outcome: the sheet is still live.
            assert_eq!(poll_once(&mut presentation), Poll::Pending);
            assert!(live_since().is_some());
        });
    }

    #[test]
    fn a_lost_host_resolves_the_sheet_and_frees_the_slot() {
        isolated_sheet(|| {
            let mut presentation = show_sheet_on::<FakeSheetHost>(sheet_spec());
            assert!(sheet_resolves(Ok(SheetOutcome::HostLost)));
            assert_eq!(
                poll_once(&mut presentation),
                Poll::Ready(Ok(SheetOutcome::HostLost))
            );
            assert!(live_since().is_none());
        });
    }

    #[test]
    fn alerts_and_sheets_share_the_one_busy_slot() {
        isolated_sheet(|| {
            let alert = show_alert_on::<FakeHost>(two_button_spec());
            let mut sheet = show_sheet_on::<FakeSheetHost>(sheet_spec());
            assert_eq!(sheet.sheet_handle(), None);
            assert_eq!(poll_once(&mut sheet), Poll::Ready(Err(PresentError::Busy)));
            assert_eq!(FAKE_SHEET.with(|fake| fake.borrow().shown), 0);
            drop(alert);

            let live_sheet = show_sheet_on::<FakeSheetHost>(sheet_spec());
            assert!(live_sheet.sheet_handle().is_some());
            let mut alert = show_alert_on::<FakeHost>(two_button_spec());
            assert_eq!(poll_once(&mut alert), Poll::Ready(Err(PresentError::Busy)));

            // Resolving the sheet frees the slot for the next kind at once.
            assert!(sheet_resolves(Ok(SheetOutcome::Dismissed(
                DismissReason::User
            ))));
            let mut next = show_alert_on::<FakeHost>(two_button_spec());
            assert!(next.handle().is_some());
            assert_eq!(poll_once(&mut next), Poll::Pending);
            drop(live_sheet);
        });
    }

    #[test]
    fn the_detent_listener_is_carried_and_compared_by_identity() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let spec = sheet_spec().on_detent(move |detent| {
            sink.lock().expect("unpoisoned").push(detent);
        });
        let listener = spec.detent_listener().expect("registered");
        listener(Detent::Large);
        listener(Detent::Custom(0.4));
        assert_eq!(
            *seen.lock().expect("unpoisoned"),
            vec![Detent::Large, Detent::Custom(0.4)]
        );
        assert_eq!(spec.clone(), spec, "a clone shares the listener");
        assert_ne!(spec.clone().on_detent(|_| {}), spec);
        assert!(sheet_spec().detent_listener().is_none());
    }

    // --- Kotlin <-> Rust drift checks for the presenter ------------------
    //
    // `FrustNativePresenter.kt` and `android_host.rs` are one contract: the
    // Kotlin package + class are baked into the Rust export's mangled symbol
    // and into the binary class name Rust loads, and the outcome codes are
    // shared integers. Neither side can be compiled against the other on a
    // host, so these read both sources as text.

    const PRESENTER_KT: &str = include_str!(
        "../../platform/android/src/main/kotlin/dev/frust/nativewidgets/FrustNativePresenter.kt"
    );
    const ANDROID_HOST_RS: &str = include_str!("android_host.rs");

    fn kotlin_package() -> &'static str {
        PRESENTER_KT
            .lines()
            .find_map(|line| line.trim().strip_prefix("package "))
            .expect("FrustNativePresenter.kt declares a package")
            .trim()
    }

    fn kotlin_object_name() -> String {
        PRESENTER_KT
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("object ")
                    .filter(|rest| rest.starts_with(char::is_uppercase))
            })
            .expect("FrustNativePresenter.kt declares a named `object`")
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    }

    #[test]
    fn presenter_outcome_codes_match_between_kotlin_and_rust() {
        let mut kotlin = BTreeMap::new();
        for line in PRESENTER_KT.lines() {
            let Some(rest) = line.trim().strip_prefix("const val OUTCOME_") else {
                continue;
            };
            let (name, value) = rest.split_once('=').expect("`NAME = value`");
            let value: i32 = value
                .trim()
                .parse()
                .unwrap_or_else(|e| panic!("OUTCOME_{name}: {e}"));
            kotlin.insert(name.trim().to_string(), value);
        }
        let rust = BTreeMap::from([
            ("ACTION".to_string(), wire::OUTCOME_ACTION),
            ("CANCELLED".to_string(), wire::OUTCOME_CANCELLED),
            ("DISMISSED".to_string(), wire::OUTCOME_DISMISSED),
            ("HOST_LOST".to_string(), wire::OUTCOME_HOST_LOST),
            ("NO_HOST".to_string(), wire::OUTCOME_NO_HOST),
            ("FAILED".to_string(), wire::OUTCOME_FAILED),
        ]);
        assert_eq!(
            kotlin, rust,
            "FrustNativePresenter.kt's OUTCOME_* constants and present::wire must be edited \
             together"
        );
    }

    #[test]
    fn presenter_class_and_export_match_the_kotlin_declaration() {
        let package = kotlin_package();
        let object = kotlin_object_name();

        let binary = format!("\"{package}.{object}\"");
        assert!(
            ANDROID_HOST_RS.contains(&format!("const PRESENTER_CLASS_BINARY: &str = {binary};")),
            "android_host.rs's PRESENTER_CLASS_BINARY must be {binary}"
        );

        assert!(
            PRESENTER_KT.contains(
                "external fun nativeOnOutcome(generation: Long, code: Int, actionIndex: Int)"
            ),
            "FrustNativePresenter.kt must declare the nativeOnOutcome(Long, Int, Int) callback"
        );
        let symbol = format!(
            "pub extern \"system\" fn Java_{}_{object}_nativeOnOutcome",
            package.replace('.', "_")
        );
        assert!(
            ANDROID_HOST_RS.contains(&symbol),
            "android_host.rs must export `{symbol}` — the JVM binds `external` methods by \
             mangled name alone"
        );
    }
}
