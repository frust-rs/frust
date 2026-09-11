//! The iOS native video view: ONE Rust `define_class!` Objective-C factory
//! implementing the embedding's `FrustPlatformViewFactory` protocol, plus the
//! `UIView` subclass it builds — **zero Swift**, and no C export. The factory
//! reaches its session's player through [`crate::apple::player_for`], the only
//! seam that module publishes to a view.
//!
//! `plugins/native-widgets/src/apple/factory.rs` is the in-repo precedent this
//! module follows: the same protocol declaration, the same name pins, the same
//! non-nil failure contract, the same `catch_unwind` discipline, and the same
//! lazy-registration hook. Read that module's doc for the reasoning behind
//! each; only what differs for a *video* slot is restated here.
//!
//! # The frozen ObjC ↔ Rust contract
//!
//! | Protocol method (Swift) | ObjC selector | This module |
//! |---|---|---|
//! | `createView(paramsJson:) -> UIView` (**required**) | `createViewWithParamsJson:` | [`VideoPlayerViewFactory::createView_paramsJson`] |
//! | `updateParams(_:paramsJson:)` (optional) | `updateParams:paramsJson:` | [`VideoPlayerViewFactory::update_params`] |
//! | `disposeView(_:)` (optional) | `disposeView:` | [`VideoPlayerViewFactory::dispose_view`] |
//!
//! All three selector spellings are taken verbatim from the Swift declaration
//! in
//! `platform/ios/FrustEmbedding/Sources/FrustEmbedding/FrustPlatformViewFactory.swift`
//! (Swift derives `createViewWithParamsJson:` from `createView(paramsJson:)`);
//! the runtime dispatches on nothing else, so a typo here is a silently dead
//! slot.
//!
//! [`VideoPlayerViewFactory::createView_paramsJson`]: VideoPlayerViewFactory
//! [`VideoPlayerViewFactory::update_params`]: VideoPlayerViewFactory
//! [`VideoPlayerViewFactory::dispose_view`]: VideoPlayerViewFactory
//!
//! ## Two name pins, both load-bearing
//!
//! 1. **The factory class name is pinned** to [`FACTORY_CLASS_NAME`] through
//!    `define_class!`'s `#[name = "…"]`, because `FrustViewHost.resolveFactory`
//!    resolves it with `NSClassFromString([`crate::VIEW_TYPE`])` — objc2's
//!    otherwise-recommended auto-generated, version-bearing name would break
//!    that lookup on every version bump. The *drift guard* below is what keeps
//!    the literal and the constant equal.
//! 2. **The protocol name is pinned** to `"FrustPlatformViewFactory"` through
//!    [`extern_protocol!`]'s own `#[name = "…"]`, matching the Swift
//!    `@objc(FrustPlatformViewFactory)` annotation. Without both halves,
//!    `class_conformsToProtocol` sees nothing and the host rejects the factory.
//!
//! The hosted view class, by contrast, is deliberately **not** named: nothing
//! ever looks it up by name (it is only constructed from live Rust code here),
//! so it keeps objc2's auto-generated version-bearing name and stays safe to
//! coexist with a SemVer-incompatible copy of this crate in one binary.
//!
//! Declaring `FrustPlatformViewFactory` here as well as in
//! `frust-native-widgets` is not a conflict: [`extern_protocol!`] only *looks a
//! protocol up* by name at runtime, it never registers one, so both crates
//! resolve the single protocol object Swift registered.
//!
//! ## The failure contract: a placeholder view, never a nil return
//!
//! `createView(paramsJson:)` is declared non-optional in Swift and
//! `FrustViewHost.applyCreate` calls `addSubview` on the result with no nil
//! check — a guarantee the compiler cannot enforce across an
//! Objective-C-implemented method. So this method returns a view on **every**
//! path, and which view depends on how the create failed:
//!
//! - **Unreadable params, no session field, or no live player for the id** — a
//!   fully-built, empty [`VideoPlayerView`]: black, with a player layer
//!   attached to nothing. This is deliberate, and it is
//!   [`crate::apple::player_for`]'s own frozen instruction: a factory handed
//!   `None` should attach nothing and wait for the next update rather than
//!   treat it as an error, because `None` also covers a session whose player is
//!   still being constructed. Collapsing that into an inert `UIView` would make
//!   the slot permanently unrecoverable, since `updateParams:paramsJson:` would
//!   then have no player layer to attach to.
//! - **A caught panic** — a bare, inert [`UIView`] ([`dead_slot_view`]). The
//!   half-built view's own state is untrustworthy at that point, so the slot is
//!   surrendered rather than reused; a later `updateParams:` finds no
//!   [`VideoPlayerView`] and no-ops.
//!
//! Either way the host gets an invisible, inert rectangle it can position and
//! hide like any other — never nil.
//!
//! ## `+layerClass`, not a sublayer
//!
//! [`VideoPlayerView`] overrides the `+layerClass` **class method** to return
//! `AVPlayerLayer`, so the view's own backing layer *is* the player layer. The
//! alternative — a plain `UIView` with an `AVPlayerLayer` added as a sublayer
//! and resized from `layoutSubviews`, which is what `plugins/camera`'s
//! `CameraPreviewView` does — was rejected here:
//!
//! - UIKit keeps a backing layer at the view's bounds itself, so there is no
//!   `layoutSubviews` override, no manual frame arithmetic, and no
//!   one-frame-stale picture during a resize animation.
//! - The host owns slot geometry entirely (the factory protocol's contract),
//!   and a backing layer inherits every geometry change for free.
//!
//! The cost is that the layer is reached by message send rather than through
//! `UIView::layer` — see [`VideoPlayerView::player_layer`].
//!
//! ## Detach on dispose, never a teardown
//!
//! `disposeView:` sets the layer's player to nil and stops there: the
//! `AVPlayer` keeps running, and only [`crate::PlayerSession::close`] tears a
//! session down. This is `plugins/camera`'s preview-vs-session lifetime split
//! (`CameraPreviewFactory.disposeView`) applied to playback, and it is what
//! lets an app navigate away from a video slot and back without re-opening —
//! or, more sharply, keeps audio playing across that navigation instead of
//! killing it as a side effect of a view disappearing.
//!
//! ## Registration is LAZY — and must be forced
//!
//! objc2 registers a `define_class!` class with the Objective-C runtime lazily,
//! on the first Rust call to `ClassType::class()`, and nothing on the host side
//! can trigger it: `NSClassFromString` returns nil for a class that was never
//! registered. [`ensure_registered`] is that trigger, and `crate::apple`'s
//! backend calls it unconditionally from every open — always before a session
//! exists, therefore always before the slot naming one can publish its first
//! `Create`.
//!
//! ## The drift guard
//!
//! Three spellings must agree: `define_class!`'s `#[name = "…"]`,
//! [`FACTORY_CLASS_NAME`], and [`crate::VIEW_TYPE`]. A mismatch is silent —
//! `NSClassFromString` returns nil, the host marks the slot dead, and the video
//! renders nothing at all — so they are pinned together in layers:
//!
//! - [`FACTORY_CLASS_NAME`] against [`crate::VIEW_TYPE`] and
//!   [`crate::contract::APPLE_VIEW_TYPE`]: a **`const` assertion**, evaluated by
//!   `cargo check` for an Apple target itself. No test harness, no simulator,
//!   and no way to build the crate through it.
//! - `#[name = "…"]` against [`FACTORY_CLASS_NAME`]: the attribute takes a
//!   literal and cannot reference a constant, so this pair is checked where the
//!   answer is real — [`ensure_registered`] reads the *registered* class's own
//!   name back out of the runtime and logs loudly if it differs, and this
//!   module's test suite asserts the same thing.
//!
//! Those tests run only where this module compiles, which is `target_os = "ios"`
//! (the crate root's `mod ios_view` gate). They therefore do **not** run in a
//! Linux or macOS host `cargo test`, and this module cannot change that — the
//! gate lives in the crate root. The host-runnable half of the guard already
//! exists one layer up and is unaffected: `crate::conformance`'s
//! `the_view_type_is_target_gated_never_one_shared_literal` pins
//! [`crate::contract::APPLE_VIEW_TYPE`] to its literal on every target. With
//! the `const` assertion joining [`FACTORY_CLASS_NAME`] to that same constant
//! at compile time, the whole chain is covered without a host test here.
//!
//! # No unwind across the ObjC boundary
//!
//! Every method the runtime can enter runs inside
//! `catch_unwind(AssertUnwindSafe(…))`, this crate's half of
//! `docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule (and the same rule
//! `crate::apple`'s main-thread hop implements for the session side).
//! `createViewWithParamsJson:` acquires its [`MainThreadMarker`] *outside* the
//! guard, because it must return a `UIView` on every path and its failure arm
//! needs a marker too; the other two methods need no marker at all, so they
//! have no such exception. Under `debug_assertions` `MainThreadMarker::from`
//! asserts, which can only fire once the host has already violated the
//! main-thread contract, and the release profile is `panic = "abort"`.

// Under `cfg(test)` `crate::backend::Active` selects the in-memory mock on
// every target, Apple included, so `crate::apple`'s backend — the only caller
// of `ensure_registered` — is unreachable in a test build (that module carries
// the same allowance for the same reason).
#![cfg_attr(test, allow(dead_code))]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyProtocol, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, extern_protocol, msg_send};
use objc2_av_foundation::{
    AVLayerVideoGravity, AVLayerVideoGravityResizeAspect, AVLayerVideoGravityResizeAspectFill,
    AVPlayer, AVPlayerLayer,
};
use objc2_foundation::NSString;
use objc2_quartz_core::CALayer;
use objc2_ui_kit::{UIColor, UIView};

use crate::VideoFit;
use crate::apple::player_for;

/// The Objective-C runtime name the factory class registers under — the string
/// `FrustViewHost.resolveFactory` feeds to `NSClassFromString`, and therefore
/// byte-identical to [`crate::VIEW_TYPE`] on this target (the module doc's
/// *drift guard*, which enforces that rather than trusting it).
///
/// Kept as a named constant even though `define_class!`'s `#[name = "…"]` needs
/// a literal and cannot reference one: this is where the pin is explained, and
/// it is what [`ensure_registered`]'s receipt compares the runtime's answer
/// against.
pub(crate) const FACTORY_CLASS_NAME: &str = "VideoPlayerViewFactory";

/// The Objective-C runtime name of the embedding's factory protocol — pinned on
/// both sides (the module doc's *Two name pins*).
const FACTORY_PROTOCOL_NAME: &std::ffi::CStr = c"FrustPlatformViewFactory";

// The `const` half of the module doc's *drift guard*: `cargo check` for an
// Apple target refuses to build this module if the factory's published name
// ever stops matching the `viewType` an app is told to name the slot with.
const _: () = assert!(
    str_eq(FACTORY_CLASS_NAME, crate::contract::APPLE_VIEW_TYPE),
    "the iOS factory class name must equal `contract::APPLE_VIEW_TYPE`"
);
const _: () = assert!(
    str_eq(FACTORY_CLASS_NAME, crate::VIEW_TYPE),
    "the iOS factory class name must equal the published `VIEW_TYPE`"
);

/// Byte equality for two `&str`, usable in a `const` assertion.
///
/// `str`'s own `PartialEq` is not `const`, and the drift guard above is worth
/// more as a compile error than as a test only a simulator ever runs.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

extern_protocol!(
    /// The FrustEmbedding Swift package's `@objc(FrustPlatformViewFactory)
    /// public protocol`, declared here by **runtime name** so the Rust class
    /// below can register a conformance the host's `class_conformsToProtocol`
    /// check actually sees.
    ///
    /// Only the required method is declared. The two optional ones are
    /// implemented as plain methods on the class carrying the protocol's own
    /// selectors — objc2's `#[optional]` attribute has no effect inside
    /// `define_class!` yet, and the selector is all the runtime dispatches on.
    ///
    /// # Safety
    ///
    /// Selector and types mirror the Swift declaration exactly:
    /// `createView(paramsJson: String) -> UIView` ⇒ `createViewWithParamsJson:`
    /// taking an `NSString` and returning a **nonnull** `UIView` — the module
    /// doc's *failure contract* is what keeps that "nonnull" honest.
    // The `# Safety` section above is present and rendered, but clippy reads a
    // doc comment that arrived through a macro's token stream as empty (every
    // fragment's span is "from expansion"), so `missing_safety_doc` fires on
    // the expanded trait whatever the source says. Silenced at the single item
    // it is wrong about rather than module-wide.
    #[allow(clippy::missing_safety_doc)]
    #[name = "FrustPlatformViewFactory"]
    pub(crate) unsafe trait FrustPlatformViewFactory: NSObjectProtocol {
        #[unsafe(method(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView>;
    }
);

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - No ivars, and no `Drop` impl, so the macro generates no `dealloc`.
    #[unsafe(super(NSObject))]
    // The host resolves, instantiates and drives this factory on the main
    // thread (the protocol's documented contract, and `FrustViewHost`'s own
    // `CADisplayLink` poll), and every UIKit and `AVPlayerLayer` call below is
    // main-thread-only — so is this class. That is what makes
    // `MainThreadMarker::from(self)` a compile-time proof rather than a check.
    #[thread_kind = MainThreadOnly]
    // Must equal `FACTORY_CLASS_NAME`; see the module doc's *drift guard* for
    // what checks that, and why the attribute cannot name the constant.
    #[name = "VideoPlayerViewFactory"]
    pub(crate) struct VideoPlayerViewFactory;

    unsafe impl NSObjectProtocol for VideoPlayerViewFactory {}

    // SAFETY: implements the protocol's one required method with the declared
    // signature; the host calls it on the main thread.
    unsafe impl FrustPlatformViewFactory for VideoPlayerViewFactory {
        /// `createView(paramsJson:)` → a [`VideoPlayerView`] attached to the
        /// session the params name.
        ///
        /// Never nil, on any path — the module doc's *failure contract* is
        /// which view each failure yields, and why the two differ.
        #[unsafe(method_id(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView> {
            // Deliberately OUTSIDE the guard, unlike the two methods below:
            // this method must return a `UIView` on every path, so the panic
            // arm needs a marker too, and acquiring one there would re-run the
            // call that just panicked with nothing left to catch it.
            let mtm = MainThreadMarker::from(self);
            let created = catch_unwind(AssertUnwindSafe(|| {
                let view = VideoPlayerView::new(mtm);
                view.apply_params(&params_json.to_string(), "createView");
                Retained::into_super(view)
            }));
            created.unwrap_or_else(|_| {
                log::warn!("{PANIC_MESSAGE_CREATE}");
                dead_slot_view(mtm)
            })
        }
    }

    impl VideoPlayerViewFactory {
        /// `updateParams(_:paramsJson:)` → re-attach the SAME player layer to
        /// whichever session the new params name, and re-apply the fit.
        ///
        /// A params-only change never tears the slot down: a session id that
        /// changed (a source swapped behind one slot) re-points the existing
        /// `AVPlayerLayer`, exactly as `plugins/camera`'s
        /// `CameraPreviewFactory.updateParams` re-points its preview layer.
        ///
        /// A `view` this factory did not create is tolerated, not an error — it
        /// is what the caught-panic arm of `createView` leaves behind.
        #[unsafe(method(updateParams:paramsJson:))]
        fn update_params(&self, view: &UIView, params_json: &NSString) {
            // No `MainThreadMarker` is needed here or in `disposeView:` —
            // nothing is constructed — so unlike `createView` there is no
            // marker to acquire, and therefore no reason to leave the guard.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                match view.downcast_ref::<VideoPlayerView>() {
                    Some(view) => view.apply_params(&params_json.to_string(), "updateParams"),
                    None => log::debug!(
                        "frust-video-player: updateParams for a view this factory did not \
                         create — ignored"
                    ),
                }
            }));
            if outcome.is_err() {
                log::warn!("{PANIC_MESSAGE_UPDATE}");
            }
        }

        /// `disposeView(_:)` → detach the layer from its player, and stop.
        ///
        /// The `AVPlayer` keeps running: the module doc's *Detach on dispose*.
        /// Only `crate::PlayerSession::close` tears a session down.
        #[unsafe(method(disposeView:))]
        fn dispose_view(&self, view: &UIView) {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                match view.downcast_ref::<VideoPlayerView>() {
                    Some(view) => view.detach(),
                    None => log::debug!(
                        "frust-video-player: disposeView for a view this factory did not \
                         create — ignored"
                    ),
                }
            }));
            if outcome.is_err() {
                log::warn!("{PANIC_MESSAGE_DISPOSE}");
            }
        }
    }
);

define_class!(
    // SAFETY:
    // - `UIView` is designed for subclassing; this subclass overrides only
    //   `+layerClass`, whose contract is to name a `CALayer` subclass.
    // - No ivars, and no `Drop` impl, so the macro generates no `dealloc`.
    #[unsafe(super(UIView))]
    // UIKit is main-thread-only, and so is every `AVPlayerLayer` message this
    // view sends.
    #[thread_kind = MainThreadOnly]
    // No `#[name = "…"]`, unlike the factory: nothing resolves this class by
    // name, so objc2's auto-generated version-bearing name is the right choice
    // here (the module doc's *Two name pins*).
    pub(crate) struct VideoPlayerView;

    unsafe impl NSObjectProtocol for VideoPlayerView {}

    impl VideoPlayerView {
        /// `+layerClass` → `AVPlayerLayer`, so this view's own backing layer
        /// *is* the player layer (the module doc's *`+layerClass`, not a
        /// sublayer*). UIKit reads this once per instance while building the
        /// backing layer, and keeps that layer at the view's bounds from then
        /// on with no help from this class.
        ///
        /// An associated function rather than a method, which is how
        /// `define_class!` spells an Objective-C **class** method.
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            AVPlayerLayer::class()
        }
    }
);

impl VideoPlayerView {
    /// A fresh, black, player-less video view.
    ///
    /// Black rather than the default nil background so the letterbox bars of a
    /// `contain` fit — and the gap before the first decoded frame — read as
    /// part of the picture instead of showing whatever the host's wrapper is
    /// over. The frame is never set here: the host owns slot geometry (the
    /// factory protocol's contract).
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `-[UIView init]` (the superclass's initializer, which forwards
        // to `-initWithFrame:` with a zero rect) on a freshly allocated instance
        // of a class that declares no ivars needing initialization first.
        let this: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), init] };
        this.setBackgroundColor(Some(&UIColor::blackColor()));
        this
    }

    /// This view's backing layer, which `+layerClass` pins to `AVPlayerLayer`.
    ///
    /// Reached by message send rather than through `UIView::layer`, because that
    /// binding is `#[cfg(feature = "objc2-quartz-core")]`-gated in objc2-ui-kit
    /// and this crate's `Cargo.toml` does not enable that feature on it (it
    /// enables it on `objc2-av-foundation`, where `AVPlayerLayer` itself lives).
    /// Widening the manifest for one accessor buys nothing the message send does
    /// not already give.
    ///
    /// The downcast is checked rather than assumed: `None` means something
    /// replaced the backing layer out from under the `+layerClass` override,
    /// which every caller below treats as "attach nothing" rather than
    /// misinterpreting a foreign layer.
    fn player_layer(&self) -> Option<Retained<AVPlayerLayer>> {
        // SAFETY: `-[UIView layer]` takes no arguments and returns a non-nil
        // `CALayer *` at +0; it is main-thread-only, which this type's
        // `MainThreadOnly` kind already guarantees.
        let layer: Option<Retained<CALayer>> = unsafe { msg_send![self, layer] };
        match layer?.downcast::<AVPlayerLayer>() {
            Ok(layer) => Some(layer),
            Err(_) => {
                log::error!(
                    "frust-video-player: the video view's backing layer is not an AVPlayerLayer \
                     — the +layerClass override was bypassed; the slot will stay empty"
                );
                None
            }
        }
    }

    /// Point this view's layer at `player`, and apply `fit`.
    fn attach(&self, player: &AVPlayer, fit: VideoFit) {
        let Some(layer) = self.player_layer() else {
            return;
        };
        // SAFETY: `-setPlayer:` on this view's own layer, on the main thread.
        // objc2 marks it unsafe only because AVFoundation's header leaves the
        // argument's nullability unannotated; this one is a live, retained
        // `AVPlayer` handed over by `player_for`.
        unsafe { layer.setPlayer(Some(player)) };
        set_gravity(&layer, fit);
    }

    /// Re-apply `fit` without touching which player is attached — the whole
    /// body of a fit-only params change.
    fn set_fit(&self, fit: VideoFit) {
        if let Some(layer) = self.player_layer() {
            set_gravity(&layer, fit);
        }
    }

    /// Detach the layer from its player, leaving the player itself running (the
    /// module doc's *Detach on dispose*).
    fn detach(&self) {
        let Some(layer) = self.player_layer() else {
            return;
        };
        // SAFETY: as `attach`'s `setPlayer:`. Nil is the documented way to
        // detach a player layer, and it tears down nothing the player owns.
        unsafe { layer.setPlayer(None) };
    }

    /// The shared `createView`/`updateParams` body: read `fit` and the session
    /// id out of `params`, apply the fit unconditionally, and attach the named
    /// session's player when there is one.
    ///
    /// The fit is applied even when the session cannot be resolved, so a payload
    /// that changed only the fit still takes effect. A session that resolves to
    /// no player is **not** a detach: [`crate::apple::player_for`] answers
    /// `None` for a session whose player is still being constructed as well as
    /// for an unknown one, so the layer keeps whatever it already had and waits
    /// for the next update — that accessor's own frozen contract.
    ///
    /// `which` names the calling selector for the log lines only; it carries no
    /// dispatch meaning.
    fn apply_params(&self, params: &str, which: &str) {
        let fit = parse_fit(params, which);
        self.set_fit(fit);

        let Some(session) = parse_session(params) else {
            log::warn!(
                "frust-video-player: {which} params carry no readable `{}` field — the video \
                 slot stays empty until the next update",
                crate::SESSION_KEY
            );
            return;
        };
        match player_for(session) {
            Some(player) => {
                self.attach(&player, fit);
                log::debug!("frust-video-player: {which} attached session {session}");
            }
            None => log::warn!(
                "frust-video-player: {which} found no live player for session {session} — the \
                 video slot keeps its current picture until the next update"
            ),
        }
    }
}

/// Force the lazy Objective-C-runtime registration of the factory class, so the
/// host's `NSClassFromString([`FACTORY_CLASS_NAME`])` can find it, and report
/// what the runtime actually ended up with.
///
/// **Ordering contract**: this must run before the host polls the first `Create`
/// for one of this crate's slots. `crate::apple`'s backend calls it from every
/// open, which is always before a session — and therefore before any slot naming
/// one — exists. Idempotent and cheap after the first call (a `Once`, over
/// objc2's own one-shot registration).
///
/// Prints a one-line receipt to **stderr**, once per process, mirroring
/// `frust-native-widgets`' factory: `eprintln!` rather than `log::info!` because
/// a release build carries a log ceiling that compiles info-level logs out
/// entirely, and this receipt is *the* artifact a device gate can collect
/// (through `xcrun devicectl … --console` or `simctl launch --console-pty`) to
/// prove the zero-Swift factory really did register under the expected name and
/// protocol. It is this crate's one deliberate exception to its own "backends
/// log, never print" rule.
pub(crate) fn ensure_registered() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let class = VideoPlayerViewFactory::class();
        let protocol = AnyProtocol::get(FACTORY_PROTOCOL_NAME);
        let conforms = protocol.is_some_and(|protocol| class.conforms_to(protocol));
        let named_as_published = class.name().to_bytes() == FACTORY_CLASS_NAME.as_bytes();
        eprintln!(
            "frust-video-player: registered ObjC class {:?} (expected {FACTORY_CLASS_NAME:?}); \
             protocol {FACTORY_PROTOCOL_NAME:?} found: {}; conforms: {conforms}",
            class.name(),
            protocol.is_some(),
        );
        if !named_as_published {
            // The runtime half of the module doc's *drift guard*: the
            // `#[name = "…"]` attribute takes a literal and cannot reference
            // `FACTORY_CLASS_NAME`, so this is where the two meet for real.
            log::error!(
                "frust-video-player: the ObjC factory registered as {:?}, not \
                 {FACTORY_CLASS_NAME:?} — NSClassFromString will not find it and every video \
                 slot on iOS will render nothing; the define_class! #[name] literal has drifted",
                class.name(),
            );
        }
        if !conforms {
            log::error!(
                "frust-video-player: the ObjC factory class does NOT conform to \
                 {FACTORY_PROTOCOL_NAME:?} — FrustViewHost will reject it and every video slot \
                 will render nothing; check that FrustPlatformViewFactory.swift still carries \
                 its explicit @objc(FrustPlatformViewFactory) name"
            );
        }
    });
}

/// The inert placeholder the caught-panic arm of `createViewWithParamsJson:`
/// returns instead of nil (the module doc's *failure contract*).
///
/// A bare `UIView` has a nil background and no subviews, so the host positions
/// an invisible, inert rectangle. Its frame is never set here — the host owns
/// slot geometry.
fn dead_slot_view(mtm: MainThreadMarker) -> Retained<UIView> {
    UIView::new(mtm)
}

/// Apply `fit` to `layer` as the matching `AVLayerVideoGravity`.
fn set_gravity(layer: &AVPlayerLayer, fit: VideoFit) {
    // SAFETY: reading an `extern` AVFoundation constant static (edition-2024
    // unsafe); linker-provided and non-null wherever AVFoundation is linked.
    let gravity: Option<&AVLayerVideoGravity> = unsafe {
        match fit {
            VideoFit::Contain => AVLayerVideoGravityResizeAspect,
            VideoFit::Cover => AVLayerVideoGravityResizeAspectFill,
        }
    };
    let Some(gravity) = gravity else {
        // Unreachable while AVFoundation is linked. The layer keeps its own
        // default — `resizeAspect`, which is this crate's `Contain` — so a
        // missing constant degrades to the documented default instead of
        // failing the attach.
        log::error!(
            "frust-video-player: an AVLayerVideoGravity constant resolved to null — leaving the \
             layer's own default gravity in place"
        );
        return;
    };
    // SAFETY: `-setVideoGravity:` with one of AVFoundation's own gravity
    // constants, on the main thread. objc2 marks it unsafe only because the
    // header leaves the argument's nullability unannotated.
    unsafe { layer.setVideoGravity(gravity) };
}

/// The session id a params payload names, or `None` when the field is missing
/// or is not an `i32`.
///
/// Signed and tolerant on purpose: the id arrives from the native side as
/// whatever the payload carried, and [`crate::apple::player_for`] answers `None`
/// for anything it does not know.
fn parse_session(params: &str) -> Option<i32> {
    json_field(params, crate::SESSION_KEY)?.parse().ok()
}

/// The fit a params payload names, defaulting to [`VideoFit::default`] when the
/// field is missing, and warning when it is present but unrecognized.
///
/// Matched against [`VideoFit`]'s own spellings rather than re-written literals,
/// so the two halves of the frozen params contract cannot drift.
fn parse_fit(params: &str, which: &str) -> VideoFit {
    let Some(raw) = json_field(params, FIT_KEY) else {
        return VideoFit::default();
    };
    if raw == VideoFit::Contain.as_str() {
        VideoFit::Contain
    } else if raw == VideoFit::Cover.as_str() {
        VideoFit::Cover
    } else {
        log::warn!(
            "frust-video-player: {which} params carry an unrecognized fit {raw:?} — falling back \
             to {:?}",
            VideoFit::default().as_str()
        );
        VideoFit::default()
    }
}

/// The params key naming how the picture fills its slot.
///
/// Spelled here rather than borrowed from `crate::params_json_with`, which
/// writes it inline in a `format!` template with no constant to share. It is
/// frozen either way (the crate root's account of the params spelling), and the
/// test asserting this factory reads back what that function writes is what
/// keeps the reader honest against the writer.
const FIT_KEY: &str = "fit";

/// The raw text of `key`'s value in a flat JSON object, or `None`.
///
/// A deliberately minimal scan, not a JSON parser: the only payloads this
/// factory ever sees are `crate::params_json_with`'s own
/// `{"session":N,"fit":"…"}`, plus whatever an app chose to add alongside, and
/// pulling `serde` into a platform plugin for two scalar fields is not a trade
/// this crate makes.
///
/// A quoted value is returned unquoted and **unescaped** — no payload this crate
/// writes contains an escape, and a value that does simply fails to match a
/// known fit spelling or to parse as an id, which both callers already handle. A
/// hostile payload can at worst steer this to the wrong value: it cannot panic
/// (every slice goes through `str::get`), and it cannot reach a session the
/// process does not own, because [`crate::apple::player_for`] resolves ids
/// against its own live registry.
fn json_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let needle = format!("\"{key}\"");
    let mut from = 0usize;

    while let Some(found) = json.get(from..)?.find(&needle) {
        let mut i = from + found + needle.len();
        from = i;

        i = skip_whitespace(bytes, i);
        // A match not followed by `:` was somebody's value, or a longer key's
        // prefix — keep scanning rather than reporting it as this field.
        if bytes.get(i) != Some(&b':') {
            continue;
        }
        i = skip_whitespace(bytes, i + 1);

        return if bytes.get(i) == Some(&b'"') {
            let start = i + 1;
            let mut end = start;
            while let Some(&byte) = bytes.get(end) {
                match byte {
                    b'"' => break,
                    // Step over an escaped character, so a `\"` inside the
                    // value does not end it early.
                    b'\\' => end += 2,
                    _ => end += 1,
                }
            }
            json.get(start..end.min(bytes.len()))
        } else {
            let start = i;
            let mut end = start;
            while let Some(&byte) = bytes.get(end) {
                if byte == b',' || byte == b'}' || byte.is_ascii_whitespace() {
                    break;
                }
                end += 1;
            }
            json.get(start..end).filter(|token| !token.is_empty())
        };
    }
    None
}

/// The first index at or after `from` whose byte is not ASCII whitespace.
fn skip_whitespace(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// Caught-panic message for `createViewWithParamsJson:` — a const rather than a
/// `format!` so the panic path allocates nothing.
const PANIC_MESSAGE_CREATE: &str = "frust-video-player: panic caught in createView — returning an \
                                    empty placeholder view (dead slot)";

/// Caught-panic message for `updateParams:paramsJson:`.
const PANIC_MESSAGE_UPDATE: &str =
    "frust-video-player: panic caught in updateParams — the video slot keeps its current picture";

/// Caught-panic message for `disposeView:`.
const PANIC_MESSAGE_DISPOSE: &str =
    "frust-video-player: panic caught in disposeView — the player layer may still be attached";

#[cfg(test)]
mod tests {
    use super::*;

    /// The runtime half of the module doc's *drift guard*: what
    /// `define_class!`'s `#[name = "…"]` literal actually registered as.
    ///
    /// This runs only where this module compiles — an Apple target — because
    /// the crate root gates `mod ios_view` on `target_os = "ios"`. The `const`
    /// assertions at the top of this module cover the same chain on every
    /// `cargo check`, and `crate::conformance`'s
    /// `the_view_type_is_target_gated_never_one_shared_literal` covers the
    /// crate-root constant on every host.
    #[test]
    fn the_registered_factory_class_is_named_as_published() {
        assert_eq!(
            VideoPlayerViewFactory::class().name().to_bytes(),
            FACTORY_CLASS_NAME.as_bytes(),
        );
        assert_eq!(FACTORY_CLASS_NAME, crate::VIEW_TYPE);
        assert_eq!(FACTORY_CLASS_NAME, crate::contract::APPLE_VIEW_TYPE);
    }

    /// The factory conforms to the protocol the host checks for before it ever
    /// calls `init` — the one condition deciding whether a video slot renders
    /// at all.
    #[test]
    fn the_registered_factory_conforms_to_the_host_protocol() {
        let protocol = AnyProtocol::get(FACTORY_PROTOCOL_NAME)
            .expect("FrustEmbedding registers the factory protocol");
        assert!(VideoPlayerViewFactory::class().conforms_to(protocol));
    }

    /// The reader and the writer of the params contract agree — the whole point
    /// of parsing the payload rather than trusting its shape.
    #[test]
    fn the_params_payload_this_factory_parses_is_the_one_the_crate_writes() {
        let written = crate::params_json_with(crate::SESSION_KEY, 7, VideoFit::Cover);
        assert_eq!(parse_session(&written), Some(7));
        assert_eq!(parse_fit(&written, "test"), VideoFit::Cover);
    }

    #[test]
    fn a_missing_or_unrecognized_fit_falls_back_to_contain() {
        assert_eq!(parse_fit("{\"session\":1}", "test"), VideoFit::Contain);
        assert_eq!(
            parse_fit("{\"session\":1,\"fit\":\"\"}", "test"),
            VideoFit::Contain
        );
        assert_eq!(
            parse_fit("{\"fit\":\"stretch\"}", "test"),
            VideoFit::Contain
        );
    }

    #[test]
    fn a_session_id_is_read_through_whitespace_and_dropped_when_unreadable() {
        assert_eq!(
            parse_session("{ \"session\" : 42 , \"fit\":\"cover\" }"),
            Some(42)
        );
        assert_eq!(parse_session("{\"session\":-1}"), Some(-1));
        assert_eq!(parse_session("{\"session\":\"12\"}"), Some(12));
        assert_eq!(parse_session("{\"sessionId\":12}"), None);
        assert_eq!(parse_session("{\"session\":\"nope\"}"), None);
        assert_eq!(parse_session("{\"session\":}"), None);
        assert_eq!(parse_session(""), None);
        assert_eq!(parse_session("not json at all"), None);
    }

    /// A key spelled inside somebody else's *value* is not a field — the scan
    /// keeps looking rather than reporting the first textual match.
    #[test]
    fn a_key_spelled_inside_a_value_is_not_mistaken_for_the_field() {
        assert_eq!(
            parse_fit("{\"title\":\"fit\",\"fit\":\"cover\"}", "test"),
            VideoFit::Cover
        );
        assert_eq!(parse_session("{\"title\":\"session\"}"), None);
    }

    /// Nothing in the scan panics on a truncated or adversarial payload; this
    /// factory's whole no-unwind discipline assumes it.
    #[test]
    fn a_truncated_or_adversarial_payload_never_panics() {
        for params in [
            "{",
            "{\"fit\":",
            "{\"fit\":\"",
            "{\"fit\":\"cover",
            "{\"fit\":\"co\\\"ver\"}",
            "{\"session\":\u{1f4f9}}",
            "\u{1f4f9}\"session\":1",
        ] {
            let _ = parse_session(params);
            let _ = parse_fit(params, "test");
        }
    }
}
