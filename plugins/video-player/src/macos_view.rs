//! The macOS native video view: a Rust-defined, layer-backed `NSView`
//! subclass hosting an `AVPlayerLayer`, wrapped in a
//! [`frust_plugin::desktop::DesktopViewFactory`] that the macOS desktop
//! shell's `crates/frust-shell-macos` platform-view host resolves by
//! `view_type` (see [`crate::VIEW_TYPE`]).
//!
//! Written in Rust with `objc2`'s `define_class!`, reaching its session's
//! player through [`crate::apple::player_for`] — no C export, no Swift, and
//! nothing here is `#[unsafe(no_mangle)]`. The hosted view is an opaque
//! native sibling (`crate`'s *The picture is a platform view* section, Mode
//! A): the frust slot behind it paints nothing, and AppKit composites this
//! view's layer above the frust surface.
//!
//! # Layer-hosting choice: `setLayer`, not a sublayer
//!
//! [`VideoPlayerNSView`] makes the `AVPlayerLayer` its own backing layer
//! (`setLayer:` + `setWantsLayer(true)`) rather than adding it as a sublayer
//! resized from an overridden `layout`. That is the simpler of the two shapes
//! the card offered: AppKit already keeps a view's *backing* layer's frame in
//! sync with the view's bounds on every resize (that is what "backing layer"
//! means), so nothing here has to override `layout`, `setFrame:`, or
//! `resizeSubviewsWithOldSize:` to keep the picture filling the view — the
//! shell's own `setFrame:` calls on this view are enough. A sublayer would
//! need exactly that override to achieve the same thing, for no benefit here
//! (this view hosts nothing else — no chrome, no siblings).
//!
//! `layerContentsRedrawPolicy` is pinned to
//! [`NSViewLayerContentsRedrawPolicy::Never`] so AppKit never asks the view
//! to redraw over the player layer (there is nothing for `drawRect:` to
//! paint here — the layer *is* the content).
//!
//! # Deviation: no explicit black letterbox background (Cargo.toml unchanged)
//!
//! The card's objective asked for `CALayer::setBackgroundColor` with a
//! `CGColor` so the letterbox bars under `contain` fit are black rather than
//! see-through. That call is real on `CALayer`, but in `objc2-quartz-core`
//! 0.3 it is gated `#[cfg(feature = "objc2-core-graphics")]` — a feature
//! **not** enabled on this crate's `objc2-quartz-core` dependency (only
//! `std`, `CALayer`, `CAMediaTiming` are), and adding it is out of this
//! task's scope (task instructions: do not add objc2 features to
//! `Cargo.toml`; report instead). So this module does **not** paint a black
//! background: an uncovered letterbox area shows whatever is beneath this
//! opaque native sibling instead of black. **Reported loudly, not silently
//! dropped** — see the completion summary; the fix is a one-line Cargo.toml
//! addition (`objc2-quartz-core`'s `objc2-core-graphics` feature) plus a
//! `CALayer::setBackgroundColor(Some(&CGColor::new_srgb(0.0, 0.0, 0.0, 1.0)))`
//! call in [`VideoPlayerNSView::new`], left for whoever picks up that Cargo
//! change.
//!
//! # The retain contract with the shell (frozen — the macOS shell host must match)
//!
//! Mirrors `crates/frust-plugin/src/desktop.rs`'s documented contract
//! exactly, and `plugins/native-widgets/src/apple/factory.rs`'s Apple-side
//! precedent for the same shape:
//!
//! - [`MacosVideoFactory::create`] builds a [`VideoPlayerNSView`], retains it
//!   itself (every `Retained<T>` already carries a live retain), and hands
//!   that **same +1 retain** to the shell by consuming it with
//!   [`objc2::rc::Retained::into_raw`] and wrapping the raw pointer in a
//!   [`DesktopViewHandle`]. Nothing here calls an extra `retain` — the +1
//!   `create` promises *is* the `Retained` it already owned.
//! - The shell owns that +1 for as long as the slot lives (per
//!   `frust_plugin::desktop`'s module doc, touching it only on the main
//!   thread) and is responsible for eventually handing the identical pointer
//!   back to [`MacosVideoFactory::dispose`].
//! - [`MacosVideoFactory::dispose`] takes the +1 back with
//!   [`objc2::rc::Retained::from_raw`] on the exact pointer `create` handed
//!   out, detaches the player, and lets the `Retained` drop — one retain out,
//!   one release in, net zero. The `AVPlayer` itself is unaffected either way
//!   (crate doc's Apple accessor contract, and `apple.rs`'s A6 semantics): a
//!   view's retain of the layer's `player` property is independent of
//!   [`crate::apple::player_for`]'s own registry entry, so disposing this
//!   view never stops playback.
//! - [`MacosVideoFactory::update_params`] never touches the retain count at
//!   all — it borrows the still-shell-owned pointer as a `&VideoPlayerNSView`
//!   and mutates the existing view in place.
//!
//! The `crates/frust-shell-macos` platform-view host takes the pointer
//! `create` returns with `Retained::from_raw` while its slot lives, and hands
//! that same pointer back through `dispose` — exactly one retain exchanged,
//! matching this module's half of the accounting.
//!
//! # Main-thread-only, matching `frust_plugin::desktop`'s contract
//!
//! Every [`frust_plugin::desktop::DesktopViewFactory`] method, and therefore
//! every method on [`MacosVideoFactory`], is called by the shell only on the
//! platform main thread — the same thread `NSView`/`AVPlayerLayer` require.
//! [`VideoPlayerNSView`] is declared `MainThreadOnly` accordingly, and
//! [`MacosVideoFactory::create`] independently checks
//! [`objc2::MainThreadMarker::new`] before building one (the one call the
//! card calls out explicitly, since it is the call that would otherwise
//! construct a `MainThreadOnly` view from an unproven thread). No method here
//! blocks, and each is wrapped in `catch_unwind` so a panic cannot unwind
//! into AppKit (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule, the
//! same discipline `plugins/native-widgets/src/apple/factory.rs` documents
//! for its own three-method factory).
//!
//! # The failure contract: an empty view, never a declined create
//!
//! [`MacosVideoFactory::create`] hands the shell a fully built, `Some`
//! [`DesktopViewHandle`] on every path except one. `crates/frust-shell-macos`'s
//! platform-view host treats a declined create (`None`) as terminal for that
//! slot id: it records nothing for the slot, and every later `Update` or
//! `UpdateParams` naming that slot finds no entry and is silently ignored
//! (`ViewHost::create`'s `Ok(None)` arm and the unknown-slot early returns in
//! `update`/`update_params`) — the differ, in turn, never re-emits `Create`
//! for a live slot whose `view_type` is unchanged, only `UpdateParams` with
//! the new session id. So a factory that declines for a *recoverable*
//! condition strands that slot empty forever, with no path back.
//!
//! [`crate::apple::player_for`]'s own frozen instruction governs which
//! conditions are recoverable: `None` covers both an unknown/closed session
//! and one whose player is still being constructed, so "attach nothing and
//! wait for the next update" is the only response that keeps the slot
//! reachable. `create_view` follows that instruction exactly like
//! `ios_view.rs`'s `createView(paramsJson:)` does for the same two cases:
//!
//! - **Unreadable params, or no live player for the named session** — a
//!   fully-built, detached [`VideoPlayerNSView`] (`setPlayer(None)` on its
//!   layer): empty, not dead, and remembered as waiting for that session
//!   ([`remember_pending`]). It is attached the moment the session's player
//!   is registered — `crate::apple` calls [`on_player_ready`] from the same
//!   main-thread block that builds the player — so the wait never depends
//!   on the shell sending another update (it re-invokes this factory only
//!   when a slot's params change, and a live session's params do not). A
//!   params update that arrives in between retries the attach as well.
//! - **A call off the main thread** — the one path that still answers `None`:
//!   [`objc2::MainThreadMarker::new`] failing is a caller contract violation
//!   (the shell is documented to call every [`DesktopViewFactory`] method only
//!   from the main thread), not a recoverable session state, so there is no
//!   `VideoPlayerNSView` this call could safely construct.
//! - **A caught panic** (`MacosVideoFactory::create`'s `catch_unwind` arm) —
//!   also `None`: the half-built view's state past a panic is untrustworthy,
//!   so the slot is surrendered rather than reused, exactly as `ios_view.rs`'s
//!   panic arm surrenders to an inert placeholder instead of returning what it
//!   was building.
//!
//! # `unsafe`
//!
//! Confined to this module: `define_class!`'s `#[unsafe(super(...))]`, the
//! `initWithFrame:` super-call (`NSView`'s designated initializer, sent once
//! to a freshly allocated instance whose ivars are already set — the
//! `crates/frust-shell-macos/src/appkit_glue.rs` / `plugins/camera/src/apple.rs`
//! precedent for calling a super initializer from a `define_class!` type),
//! every AVFoundation message send (`objc2` marks all of them `unsafe`, per
//! `crate::apple`'s own `unsafe` note), reading the two `AVLayerVideoGravity`
//! `extern` statics (edition-2024 marks reading an `extern` constant static
//! unsafe), and the raw-pointer retain hand-off described above.
//!
//! # What is tested, and where
//!
//! [`field_int`]/[`field_str`]/[`parse_params`] are pure functions with no
//! ObjC dependency, host-run below. [`ensure_registered`] registering exactly
//! one factory under [`crate::VIEW_TYPE`] is also host-run (`std::sync`
//! underneath, no ObjC runtime touched by *registration* itself — only by
//! what the registered factory later does). The retain-accounting half of
//! this crate's ACCEPTANCE (`create`/`update_params`/`dispose` exercised
//! against a real `NSView`) needs a live `MainThreadMarker`, which the Rust
//! test harness never grants: every `#[test]` runs on its own worker thread,
//! never the process's actual main thread, so `MainThreadMarker::new()` is
//! `None` in every test here regardless of host. That half is therefore
//! **compile-checked only** (`cargo check --target aarch64-apple-darwin
//! --all-targets` / `cargo clippy` the same), owed to a real run on Ed's Mac
//! — this module's tests do not attempt to fake it.
//!
//! This module also does not itself run on this Linux host: it is
//! `#[cfg(target_os = "macos")]`-gated at [`crate`], so it compiles here only
//! by cross-checking against `aarch64-apple-darwin`, and its tests compile
//! but never execute outside that target.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::{Arc, Once};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSResponder, NSView, NSViewLayerContentsRedrawPolicy};
use objc2_av_foundation::{
    AVLayerVideoGravity, AVLayerVideoGravityResizeAspect, AVLayerVideoGravityResizeAspectFill,
    AVPlayer, AVPlayerLayer,
};
use objc2_foundation::NSRect;
use objc2_quartz_core::CALayer;

use frust_plugin::desktop::{
    DesktopViewFactory, DesktopViewHandle, RegisterError, register_view_factory,
};

use crate::VideoFit;

define_class!(
    // SAFETY:
    // - `NSView` (transitively `NSResponder`/`NSObject`) has no subclassing
    //   requirements this class violates.
    // - `VideoPlayerNSView` implements no `Drop`, so the macro generates no
    //   `dealloc` override; the ivars (a `Retained<AVPlayerLayer>` plus three
    //   `Cell`s) are dropped by the ordinary ivar teardown the macro already
    //   emits.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    // Created and touched only on the platform main thread (module doc's
    // *Main-thread-only* section) — the same confinement `NSView` itself
    // imposes.
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewState]
    struct VideoPlayerNSView;

    unsafe impl NSObjectProtocol for VideoPlayerNSView {}
);

/// [`VideoPlayerNSView`]'s ivars: the `AVPlayerLayer` it hosts for its whole
/// lifetime, plus the two params fields [`update_view_params`] diffs against
/// so an unchanged session/fit is a no-op rather than a redundant re-attach.
struct ViewState {
    /// The player layer this view was built around — created once in
    /// [`VideoPlayerNSView::new`] and installed as the view's own backing
    /// layer; never replaced afterward. [`VideoPlayerNSView::attach`] and
    /// [`VideoPlayerNSView::detach`] only ever change its `player` property.
    /// Kept here rather than re-derived from `NSView::layer()` on every call,
    /// since that getter answers a plain `CALayer` and would need an
    /// unchecked downcast every time.
    layer: Retained<AVPlayerLayer>,
    /// The session id most recently named in a create or params update; `0`
    /// means "no session" (`crate::apple`'s own reservation — every real id
    /// is minted from `1`). Recorded through
    /// [`VideoPlayerNSView::set_session`] even when the view ends up
    /// detached, so a later update naming the same id is recognised as
    /// "nothing changed" rather than re-parsed as new.
    session: Cell<i32>,
    /// The fit most recently applied, so a params update with an unchanged
    /// `fit` skips re-touching `videoGravity`.
    fit: Cell<VideoFit>,
    /// Whether the layer currently has a player attached. Cheaper than
    /// re-reading the layer's `player` property (a message send back into
    /// AVFoundation) on every params update, and lets
    /// [`update_view_params`] tell a genuinely empty view apart from one that
    /// is intentionally showing no session.
    attached: Cell<bool>,
}

impl VideoPlayerNSView {
    /// Build a fresh, detached view: an `AVPlayerLayer` with no player yet,
    /// installed as this view's own backing layer.
    ///
    /// Created with a zero frame — this view never sizes itself; the
    /// embedding shell positions and resizes it with its own `setFrame:`
    /// once the platform-view slot's layout is known, exactly like every
    /// other platform-view host in this workspace
    /// (`plugins/native-widgets/src/apple/factory.rs`'s dead-slot `UIView`,
    /// which is likewise frameless until the host places it).
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: a class-side AVFoundation constructor. A player layer with
        // no player is a valid, fully initialized layer that simply shows
        // nothing, and this view is routinely published to the shell in
        // exactly that state (module doc's *The failure contract*); `attach`
        // supplies a player later, or never.
        let layer = unsafe { AVPlayerLayer::playerLayerWithPlayer(None) };
        let ivars = ViewState {
            layer,
            session: Cell::new(0),
            fit: Cell::new(VideoFit::default()),
            attached: Cell::new(false),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer,
        // sent exactly once to a freshly allocated instance whose ivars are
        // already set (the `ActivationObserver`/`PlayerObserver` precedent
        // for calling a super initializer through `define_class!`).
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };

        // Module doc's *Layer-hosting choice*: assign the layer, THEN turn
        // `wantsLayer` on (the order Apple's own `NSView.wantsLayer`/
        // `NSView.layer` docs specify), then forbid AppKit from ever
        // redrawing over it.
        let layer_ref: &CALayer = &this.ivars().layer;
        this.setLayer(Some(layer_ref));
        this.setWantsLayer(true);
        this.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::Never);

        this
    }

    /// Attach `player` to this view's layer, retaining it there
    /// (`AVPlayerLayer.player` is a strong property) independently of
    /// [`crate::apple::player_for`]'s own registry entry.
    fn attach(&self, player: &AVPlayer) {
        // SAFETY: a plain property write on a layer this view owns for its
        // whole lifetime, on the main thread (module doc's *Main-thread-only*
        // section).
        unsafe { self.ivars().layer.setPlayer(Some(player)) };
        self.ivars().attached.set(true);
    }

    /// Detach whatever player is currently attached. Idempotent — detaching
    /// an already-detached view is a harmless `setPlayer(None)` on top of
    /// `None`.
    fn detach(&self) {
        // SAFETY: as `attach`.
        unsafe { self.ivars().layer.setPlayer(None) };
        self.ivars().attached.set(false);
    }

    /// Whether this view currently has a player attached
    /// (`ViewState::attached`). Cheaper than reading the layer's `player`
    /// property back, and lets a caller tell an intentionally empty view
    /// apart from a still-attached one without another AVFoundation message
    /// send.
    fn is_attached(&self) -> bool {
        self.ivars().attached.get()
    }

    /// Apply `fit`'s `videoGravity` and record it, so a later params update
    /// with the same fit is a no-op (`ViewState::fit`).
    fn set_fit(&self, fit: VideoFit) {
        self.ivars().fit.set(fit);
        match video_gravity_for(fit) {
            Some(gravity) => {
                // SAFETY: a plain property write on a layer this view owns,
                // on the main thread.
                unsafe { self.ivars().layer.setVideoGravity(gravity) };
            }
            None => log::error!(
                "frust-video-player: macos AVLayerVideoGravity constant for {fit:?} was nil — \
                 leaving the previous videoGravity in place"
            ),
        }
    }

    /// The session id most recently named by a create or a params update,
    /// or `0` for none — named, not necessarily attached (module doc's *The
    /// failure contract*).
    fn session(&self) -> i32 {
        self.ivars().session.get()
    }

    /// Record `session` as the id most recently named. Independent of
    /// [`Self::attach`]/[`Self::detach`]: a create that found no live player
    /// names the session on a still-detached view, which is what lets
    /// [`on_player_ready`] recognise it later.
    fn set_session(&self, session: i32) {
        self.ivars().session.set(session);
    }

    /// The fit most recently [`Self::set_fit`].
    fn fit(&self) -> VideoFit {
        self.ivars().fit.get()
    }
}

/// `fit`'s `AVLayerVideoGravity`, or `None` if AVFoundation's own constant
/// answered nil (never observed in practice — both statics are linker-
/// provided wherever AVFoundation is linked — but the accessor is a plain
/// `Option` read, so this is answered rather than asserted).
fn video_gravity_for(fit: VideoFit) -> Option<&'static AVLayerVideoGravity> {
    // SAFETY: reading `extern` AVFoundation constant statics (edition-2024
    // marks that unsafe); both are non-null wherever AVFoundation is linked.
    unsafe {
        match fit {
            VideoFit::Contain => AVLayerVideoGravityResizeAspect,
            VideoFit::Cover => AVLayerVideoGravityResizeAspectFill,
        }
    }
}

/// The macOS desktop-view factory for [`crate::VIEW_TYPE`]: builds, updates
/// and disposes [`VideoPlayerNSView`]s (module doc's *retain contract*).
///
/// Zero-sized and stateless by design — every session's state lives in
/// [`crate::apple`]'s own registry, resolved fresh through
/// [`crate::apple::player_for`] on each call, and every view's own state
/// lives in its `ViewState` ivars. `Send + Sync` (the trait's bound) costs
/// nothing to satisfy: there is nothing here to share incorrectly.
struct MacosVideoFactory;

impl DesktopViewFactory for MacosVideoFactory {
    /// Build a new [`VideoPlayerNSView`] for the session/fit named in
    /// `params_json`, attached when a live player already resolves and empty
    /// (but reusable) when it does not — never panic across this boundary
    /// (module doc's *Main-thread-only* and *The failure contract* sections).
    fn create(&self, params_json: &str) -> Option<DesktopViewHandle> {
        let outcome = catch_unwind(AssertUnwindSafe(|| create_view(params_json)));
        match outcome {
            Ok(handle) => handle,
            Err(_) => {
                log::warn!(
                    "frust-video-player: panic caught in macos create — surrendering the slot"
                );
                None
            }
        }
    }

    /// Re-attach on a session change, re-attach an empty view whose session
    /// resolves now, and re-apply `videoGravity` on a fit change — each
    /// diffed against the view's own `ViewState`.
    fn update_params(&self, view: &DesktopViewHandle, params_json: &str) {
        let outcome = catch_unwind(AssertUnwindSafe(|| update_view_params(view, params_json)));
        if outcome.is_err() {
            log::warn!(
                "frust-video-player: panic caught in macos update_params — the view keeps its \
                 previous session/fit"
            );
        }
    }

    /// Take the +1 retain `create` handed out back, detach, and drop
    /// (module doc's *retain contract*). The attached `AVPlayer` keeps
    /// running — disposing the view is not the same as closing the session
    /// (`crate::apple`'s A6 semantics).
    fn dispose(&self, view: DesktopViewHandle) {
        let outcome = catch_unwind(AssertUnwindSafe(|| dispose_view(view)));
        if outcome.is_err() {
            log::warn!(
                "frust-video-player: panic caught in macos dispose — the view's native \
                 references may leak"
            );
        }
    }
}

// --- Views waiting for a player ----------------------------------------------

thread_local! {
    /// Views handed to the shell before their session's player existed, keyed
    /// by session id — the other half of [`crate::apple::player_for`]'s
    /// "attach nothing and wait" instruction. Weak, so the registry never
    /// extends a view's life: the shell owns the one retain, and a view it has
    /// already disposed simply fails to load and is dropped on the next touch.
    ///
    /// Main-thread only, like every other piece of this module — a
    /// `thread_local!` rather than a `static Mutex` because a weak `NSView`
    /// reference is neither `Send` nor ever needed off the main thread.
    static PENDING: RefCell<HashMap<i32, Vec<Weak<VideoPlayerNSView>>>> =
        RefCell::new(HashMap::new());
}

/// Remember `view` as waiting for `session`'s player. Idempotent per view.
fn remember_pending(session: i32, view: &VideoPlayerNSView) {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        let waiting = pending.entry(session).or_default();
        waiting.retain(|weak| weak.load().is_some());
        let already = waiting
            .iter()
            .any(|weak| weak.load().is_some_and(|live| std::ptr::eq(&*live, view)));
        if !already {
            waiting.push(Weak::new(view));
        }
    });
}

/// Drop `view` from every waiting list — it was attached, re-pointed at
/// another session, or disposed.
fn forget_view(view: &VideoPlayerNSView) {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        pending.retain(|_, waiting| {
            waiting.retain(|weak| weak.load().is_some_and(|live| !std::ptr::eq(&*live, view)));
            !waiting.is_empty()
        });
    });
}

/// `crate::apple` calls this on the main thread the moment `session`'s
/// `AVPlayer` is registered: every view still waiting for that session is
/// attached now.
///
/// Without it a view created while the player was under construction — an
/// off-main-thread open builds the player in a dispatched block, so a slot's
/// create can land first — would stay empty for good: the shell re-invokes
/// this factory only when a slot's params change, and a live session's params
/// (its id and fit) do not change on their own.
pub(crate) fn on_player_ready(session: i32, _mtm: MainThreadMarker) {
    let waiting = PENDING.with(|pending| pending.borrow_mut().remove(&session));
    let Some(waiting) = waiting else {
        return;
    };
    let Some(player) = crate::apple::player_for(session) else {
        // Registered and torn down again before this ran: nothing to attach.
        return;
    };
    let mut attached = 0usize;
    for weak in waiting {
        let Some(view) = weak.load() else {
            continue;
        };
        if view.session() == session && !view.is_attached() {
            view.attach(&player);
            attached += 1;
        }
    }
    if attached > 0 {
        log::debug!(
            "frust-video-player: macos attached {attached} waiting view(s) to session {session}"
        );
    }
}

/// `crate::apple` calls this when `session` is torn down: nothing will ever
/// be ready for its waiting views, so the entry goes.
pub(crate) fn on_session_closed(session: i32) {
    PENDING.with(|pending| {
        pending.borrow_mut().remove(&session);
    });
}

/// The typed half of [`MacosVideoFactory::create`].
///
/// `None` only for the one unrecoverable case — an off-main-thread call
/// (module doc's *The failure contract*). An unparsable payload or an
/// unresolvable session still yields `Some`, wrapping a fully built but
/// detached [`VideoPlayerNSView`] that [`on_player_ready`] — or a later
/// [`update_view_params`] — attaches once the session's player exists.
fn create_view(params_json: &str) -> Option<DesktopViewHandle> {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!(
            "frust-video-player: macos create called off the main thread — the shell must call \
             DesktopViewFactory::create only from the main thread"
        );
        return None;
    };

    let view = VideoPlayerNSView::new(mtm);

    let Some((session, fit)) = parse_params(params_json) else {
        log::warn!(
            "frust-video-player: macos create received unparsable params {params_json:?} — the \
             video slot stays empty until the next update"
        );
        // Written, not assumed: `fit()` must only ever report a gravity this
        // module actually applied, so a later update that names the default
        // is not skipped on the strength of AVFoundation's own default.
        view.set_fit(VideoFit::default());
        return Some(publish(view));
    };

    view.set_fit(fit);
    view.set_session(session);

    match crate::apple::player_for(session) {
        Some(player) => view.attach(&player),
        None => {
            log::debug!(
                "frust-video-player: macos create found no live player for session {session} \
                 yet — the video slot stays empty until the session's player is ready"
            );
            remember_pending(session, &view);
        }
    }

    Some(publish(view))
}

/// Consume `view`'s `Retained` into the raw pointer the shell now owns a +1
/// over, wrapped in the [`DesktopViewHandle`] `create` hands back (module
/// doc's *retain contract*).
fn publish(view: Retained<VideoPlayerNSView>) -> DesktopViewHandle {
    // `Retained::into_raw` never answers null.
    let ptr = Retained::into_raw(view);
    let ptr = NonNull::new(ptr).expect("Retained::into_raw never returns null");
    // SAFETY: `ptr` is the +1-retained `VideoPlayerNSView` (an `NSView`)
    // pointer just produced above; it stays valid until this same pointer
    // reaches `dispose`, and it is touched only on the main thread (this
    // whole function already required one).
    unsafe { DesktopViewHandle::from_raw(ptr.cast()) }
}

/// The typed half of [`MacosVideoFactory::update_params`].
fn update_view_params(handle: &DesktopViewHandle, params_json: &str) {
    let Some((session, fit)) = parse_params(params_json) else {
        log::warn!(
            "frust-video-player: macos update_params received unparsable params {params_json:?}"
        );
        return;
    };

    // SAFETY: `handle` wraps a live `VideoPlayerNSView` pointer `create`
    // produced and no `dispose` has yet consumed (the shell's own contract);
    // this borrows it without taking ownership, and only on the main thread
    // (`DesktopViewFactory`'s contract, which this whole call already
    // requires).
    let view = unsafe { &*handle.as_ptr().cast::<VideoPlayerNSView>() };

    if view.session() != session {
        // Whatever session this view was waiting on, it is not this one.
        forget_view(view);
        match crate::apple::player_for(session) {
            Some(player) => view.attach(&player),
            None => {
                log::debug!(
                    "frust-video-player: macos update_params found no live player for session \
                     {session} yet — detaching until it is ready"
                );
                view.detach();
                remember_pending(session, view);
            }
        }
        view.set_session(session);
    } else if !view.is_attached() {
        // Same session as last time and the view is still empty: the player
        // was not ready at the create. [`on_player_ready`] is what closes
        // that window; this is the opportunistic retry for an update that
        // happens to arrive in between.
        match crate::apple::player_for(session) {
            Some(player) => {
                view.attach(&player);
                forget_view(view);
            }
            None => {
                log::debug!(
                    "frust-video-player: macos update_params for session {session} — its player \
                     is still not ready, the view stays empty"
                );
                remember_pending(session, view);
            }
        }
    }

    if view.fit() != fit {
        view.set_fit(fit);
    }
}

/// The typed half of [`MacosVideoFactory::dispose`].
fn dispose_view(handle: DesktopViewHandle) {
    let ptr = handle.into_raw().cast::<VideoPlayerNSView>().as_ptr();
    // SAFETY: `ptr` is exactly the pointer `create` handed to the shell with
    // a +1 retain (module doc's retain contract), and `dispose` is called at
    // most once per handle (the shell's own contract) — this reclaims that
    // same retain rather than fabricating a new one, and only on the main
    // thread (`DesktopViewFactory`'s contract).
    let view = unsafe { Retained::from_raw(ptr) };
    match view {
        Some(view) => {
            forget_view(&view);
            view.detach();
            // `view` drops here, releasing the +1 `create` handed out. The
            // attached `AVPlayer` is unaffected (module doc's retain
            // contract).
        }
        None => log::error!("frust-video-player: macos dispose received a null view pointer"),
    }
}

/// Register the macOS video-view factory with the desktop platform-view
/// registry, once.
///
/// Idempotent: the second and every later call is a cheap `Once` check.
/// [`RegisterError::AlreadyRegistered`] is logged at debug level rather than
/// treated as a problem — [`crate::apple`]'s `AppleBackend::open` calls this
/// unconditionally on every session open (module doc's *Factory registration
/// is lazy*, `crate::apple`'s own doc), so every open after the first would
/// otherwise "fail" this by design.
pub(crate) fn ensure_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if let Err(RegisterError::AlreadyRegistered(view_type)) =
            register_view_factory(crate::VIEW_TYPE, Arc::new(MacosVideoFactory))
        {
            log::debug!(
                "frust-video-player: macos view factory already registered for {view_type:?}"
            );
        }
    });
}

/// Parse `{"session":N,"fit":"contain"|"cover"}` — the frozen Apple params
/// payload [`crate::PlayerSession::params_json`] writes (`crate`'s own doc).
/// `None` only when `session` is missing or not an integer; an absent or
/// unrecognized `fit` defaults to [`VideoFit::Contain`] (`VideoFit`'s own
/// documented default), matching how a slot with no fit specified yet should
/// behave.
fn parse_params(json: &str) -> Option<(i32, VideoFit)> {
    let session = field_int(json, "session")?;
    let fit = match field_str(json, "fit") {
        Some("cover") => VideoFit::Cover,
        _ => VideoFit::Contain,
    };
    Some((session, fit))
}

/// Read `key`'s integer value out of a flat JSON object, e.g.
/// `field_int(r#"{"session":42}"#, "session") == Some(42)`. A tiny
/// hand-rolled reader rather than `serde` (this crate is a platform plugin —
/// `frust-plugin` + FFI crates only — and both ends of this payload are this
/// crate's own, the same rule `plugins/native-widgets/src/runtime.rs`'s
/// `Params` documents for its own flat-JSON reader). Deliberately narrower
/// than that reader: this payload is two fixed keys with no nesting and no
/// string escaping to worry about, so a plain substring search is enough.
fn field_int(json: &str, key: &str) -> Option<i32> {
    let marker = format!("\"{key}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Read `key`'s string value out of a flat JSON object, e.g.
/// `field_str(r#"{"fit":"cover"}"#, "fit") == Some("cover")`. See
/// [`field_int`]'s doc for why this is hand-rolled; unlike a general JSON
/// string reader this performs no escape decoding, which is sound here only
/// because both values this module ever reads (`"contain"`/`"cover"`) are
/// escape-free by construction.
fn field_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\":\"");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_plugin::desktop::lookup_view_factory;

    #[test]
    fn field_int_reads_a_positive_session_id() {
        assert_eq!(
            field_int(r#"{"session":42,"fit":"contain"}"#, "session"),
            Some(42)
        );
    }

    #[test]
    fn field_int_reads_a_negative_value() {
        assert_eq!(
            field_int(r#"{"session":-3,"fit":"cover"}"#, "session"),
            Some(-3)
        );
    }

    #[test]
    fn field_int_is_none_for_a_missing_key() {
        assert_eq!(field_int(r#"{"fit":"contain"}"#, "session"), None);
    }

    #[test]
    fn field_int_is_none_for_a_non_numeric_value() {
        assert_eq!(field_int(r#"{"session":"nope"}"#, "session"), None);
    }

    #[test]
    fn field_str_reads_the_fit_value() {
        assert_eq!(
            field_str(r#"{"session":1,"fit":"cover"}"#, "fit"),
            Some("cover")
        );
    }

    #[test]
    fn field_str_is_none_for_a_missing_key() {
        assert_eq!(field_str(r#"{"session":1}"#, "fit"), None);
    }

    #[test]
    fn parse_params_reads_session_and_fit() {
        assert_eq!(
            parse_params(r#"{"session":7,"fit":"cover"}"#),
            Some((7, VideoFit::Cover))
        );
        assert_eq!(
            parse_params(r#"{"session":7,"fit":"contain"}"#),
            Some((7, VideoFit::Contain))
        );
    }

    #[test]
    fn parse_params_defaults_to_contain_for_an_unrecognized_or_missing_fit() {
        assert_eq!(
            parse_params(r#"{"session":7,"fit":"bogus"}"#),
            Some((7, VideoFit::Contain))
        );
        assert_eq!(
            parse_params(r#"{"session":7}"#),
            Some((7, VideoFit::Contain))
        );
    }

    #[test]
    fn parse_params_is_none_without_a_session() {
        assert_eq!(parse_params(r#"{"fit":"cover"}"#), None);
    }

    /// `ensure_registered` registers exactly one factory under
    /// [`crate::VIEW_TYPE`], and is idempotent. Host-runnable on this Mac:
    /// registration is plain `std::sync`, touching no ObjC runtime state
    /// (module doc's *What is tested* section explains why the
    /// `create`/`update_params`/`dispose` retain accounting is NOT exercised
    /// here).
    #[test]
    fn ensure_registered_registers_exactly_one_factory_under_view_type() {
        ensure_registered();
        ensure_registered();
        assert!(lookup_view_factory(crate::VIEW_TYPE).is_some());
    }
}
