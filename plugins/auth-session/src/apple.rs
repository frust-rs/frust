//! The Apple (iOS + macOS) backend for [`crate::AuthSession::start`] —
//! `ASWebAuthenticationSession` (`objc2-authentication-services`), the
//! in-app browser tab RFC 8252 asks a native app to run its authorization
//! flow through.
//!
//! `target_vendor = "apple"`-gated (see `Cargo.toml`'s own comment):
//! `ASWebAuthenticationSession` itself is shared between iOS and macOS, and
//! only its presentation-context anchor differs — `objc2-ui-kit`'s
//! [`UIWindow`] vs `objc2-app-kit`'s [`NSWindow`], each a `target_os`-gated
//! dependency. [`AnchorWindow`] is that one-type difference; everything else
//! in this module is shared.
//!
//! # Main-thread dispatch
//!
//! `ASWebAuthenticationSession` presents UI, and its presentation-context
//! provider is a `MainThreadOnly` protocol (the binding's own supertrait
//! bound). [`start`] therefore builds **no** Objective-C object on the
//! caller's thread: it bounces the whole construct-configure-present
//! sequence onto `dispatch_get_main_queue()` with
//! `DispatchQueue::exec_async` and returns `Ok(())` immediately — the same
//! never-block-the-caller shape `frust-haptics`'s iOS backend uses, and the
//! reason `Ok(())` here means "the session was handed to the main queue",
//! not "the session is on screen". Every later failure reaches the caller
//! through [`crate::resolve`] instead, which is exactly what the crate's
//! awaitable contract is for.
//!
//! # Retained until completion
//!
//! `-[ASWebAuthenticationSession setPresentationContextProvider:]` is a
//! **weak** property, and the session object itself is not retained by
//! anything in UIKit/AppKit once [`start_on_main`] returns. Dropping either
//! at the end of that function would tear the session down before the user
//! ever saw it, so [`LIVE`] holds the session, its anchor, and the
//! completion block for exactly as long as the session is live. It is a
//! `thread_local!` rather than a `static` because `Retained`/`RcBlock` are
//! `!Send`, and everything that touches it already runs on the main thread.
//!
//! [`LIVE`] is populated *before* `-start`, and released on a **later** main
//! queue turn (see [`release_live`]) rather than inline in the completion
//! block: releasing the last reference to a session from inside that
//! session's own completion handler would deallocate an object whose frame
//! is still on the stack. Each entry carries a generation tag and a release
//! clears the slot only if the tag still matches, so a completion that
//! lands off the main thread — Apple documents no thread for the handler —
//! can never tear down a session the caller has already started in its
//! place.
//!
//! # The iOS 15 floor and the deprecated initializer
//!
//! `-initWithURL:callbackURLScheme:completionHandler:` is marked
//! `#[deprecated]` by the binding in favour of iOS 17.4's
//! `-initWithURL:callback:completionHandler:` (which takes an
//! `ASWebAuthenticationSessionCallback` and can express an HTTPS App-Link
//! callback as well as a custom scheme). This crate's deployment floor is
//! iOS 15, where the newer initializer does not exist at all, so
//! [`make_session`] keeps the older one behind a narrowly scoped
//! `#[allow(deprecated)]` — the same shape `frust-haptics`'s `impact` uses
//! for its own header-translator deprecation. Raising the floor to 17.4 is
//! the only thing that would change this.
//!
//! # The callback is intercepted in-process
//!
//! Apple's documented precondition still applies: an app using a custom
//! `callbackURLScheme` must register that scheme in its `Info.plist` under
//! `CFBundleURLTypes`. What does **not** happen is the app ever seeing the
//! redirect as a URL open — `ASWebAuthenticationSession` intercepts the
//! callback itself inside the browser tab it owns and hands it straight to
//! the completion handler, so no custom-scheme open reaches the app
//! delegate and `frust_on_deep_link` is **not** invoked for it. An app that
//! also routes ordinary deep links can leave that path untouched; the two
//! do not interact.
//!
//! # `unsafe`
//!
//! Every `objc2-authentication-services` method is `unsafe` in the binding
//! (nullability and pointer parameters the header-translator could not
//! prove), so this module's `unsafe` inventory is:
//!
//! - [`start`]'s one `MainThreadMarker::new_unchecked()`, proving that a
//!   `dispatch_get_main_queue()` callback runs on the real main thread
//!   (`frust-haptics`'s apple backend's identical claim).
//! - [`make_session`]'s initializer call, whose whole caller contract is
//!   the completion-block lifetime — stated on that function.
//! - [`start_on_main`]'s two property setters and its `-start` call, each
//!   a plain message send to a live session on the main thread.
//! - [`outcome_from`]'s two raw-pointer derefs plus the one `extern static`
//!   read (`ASWebAuthenticationSessionErrorDomain`), all stated there.
//! - [`Anchor`]'s `define_class!` block: its `#[unsafe(super(NSObject))]`,
//!   its `#[unsafe(method_id(…))]` selector, and the two `unsafe impl`
//!   protocol conformances share the block's one `SAFETY:` note, and
//!   [`Anchor::new`]'s `msg_send![super(this), init]` carries its own.
//!
//! No unwind may cross back into AuthenticationServices' own frames, so the
//! completion block's body runs under `catch_unwind`
//! (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule).

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
compile_error!(
    "the auth-session Apple backend targets iOS and macOS only; \
     `Cargo.toml` declares an anchor dependency for neither of the other \
     Apple platforms"
);

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_authentication_services::{
    ASPresentationAnchor, ASWebAuthenticationPresentationContextProviding,
    ASWebAuthenticationSession, ASWebAuthenticationSessionCompletionHandler,
    ASWebAuthenticationSessionErrorCode, ASWebAuthenticationSessionErrorDomain,
};
use objc2_foundation::{NSError, NSString, NSURL};

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSWindow};
#[cfg(target_os = "ios")]
use objc2_ui_kit::{UIApplication, UISceneActivationState, UIWindow, UIWindowScene};

use crate::{AuthSessionError, AuthSessionOutcome, AuthSessionRequest, SessionToken};

/// The window type `ASPresentationAnchor` actually is on this platform —
/// `UIWindow` on iOS, `NSWindow` on macOS (the binding itself erases both to
/// `NSObject`). The one platform difference in this module.
#[cfg(target_os = "ios")]
type AnchorWindow = UIWindow;
#[cfg(target_os = "macos")]
type AnchorWindow = NSWindow;

/// The one live session's generation tag plus its Objective-C objects: the
/// session, the anchor whose provider property the session holds only
/// **weakly**, and the completion block. Held as a tuple because only the
/// tag is ever read back — keeping the other three alive is the whole point
/// (module doc's *Retained until completion*).
type LiveSession = (
    u64,
    Retained<ASWebAuthenticationSession>,
    Retained<Anchor>,
    RcBlock<dyn Fn(*mut NSURL, *mut NSError)>,
);

thread_local! {
    /// The live session's objects, on the main thread that owns them.
    /// `crate::ACTIVE`'s Busy guard is what keeps this to at most one entry;
    /// this slot is purely about object lifetime, never about routing.
    static LIVE: RefCell<Option<LiveSession>> = const { RefCell::new(None) };

    /// The last generation tag handed out — see [`LiveSession`].
    static LAST_GENERATION: Cell<u64> = const { Cell::new(0) };
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivar is a plain `Retained<AnchorWindow>`, released by the
    //   macro's generated `dealloc` with nothing extra to uphold.
    // - `ASWebAuthenticationPresentationContextProviding` requires
    //   `NSObjectProtocol` + `MainThreadOnly`, both satisfied below, and its
    //   one required method is implemented with the declared signature: a
    //   presentation anchor for the asking session. The binding erases
    //   `ASPresentationAnchor` to `NSObject`, and the ivar this returns is a
    //   real `UIWindow`/`NSWindow` — the concrete type the framework
    //   documents for each platform.
    #[unsafe(super(NSObject))]
    // The protocol is `MainThreadOnly` (its own supertrait bound), and the
    // anchor is only ever built and read on the main queue.
    #[thread_kind = MainThreadOnly]
    #[name = "FrustAuthSessionAnchor"]
    #[ivars = Retained<AnchorWindow>]
    struct Anchor;

    unsafe impl NSObjectProtocol for Anchor {}

    unsafe impl ASWebAuthenticationPresentationContextProviding for Anchor {
        /// The window this session presents over, pinned at construction
        /// time rather than looked up here: the lookup can fail (no scene,
        /// no key window), and a protocol method that must return a
        /// non-null anchor is the wrong place to discover that. [`Anchor`]
        /// is only ever built from a window that already exists, so this
        /// cannot fail and needs no fallback.
        #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
        fn presentation_anchor(
            &self,
            _session: &ASWebAuthenticationSession,
        ) -> Retained<ASPresentationAnchor> {
            as_presentation_anchor(self.ivars().clone())
        }
    }
);

impl Anchor {
    /// An anchor over `window`.
    fn new(mtm: MainThreadMarker, window: Retained<AnchorWindow>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(window);
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set — the idiom
        // `plugins/native-widgets/src/apple/events.rs` and
        // `plugins/camera/src/apple.rs` both use.
        unsafe { msg_send![super(this), init] }
    }
}

/// Erase a window to the `ASPresentationAnchor` the protocol returns (the
/// binding aliases it to `NSObject` on every platform).
#[cfg(target_os = "ios")]
fn as_presentation_anchor(window: Retained<UIWindow>) -> Retained<ASPresentationAnchor> {
    // `UIWindow: UIView: UIResponder: NSObject`.
    window.into_super().into_super().into_super()
}

/// Erase a window to the `ASPresentationAnchor` the protocol returns (the
/// binding aliases it to `NSObject` on every platform).
#[cfg(target_os = "macos")]
fn as_presentation_anchor(window: Retained<NSWindow>) -> Retained<ASPresentationAnchor> {
    // `NSWindow: NSResponder: NSObject`.
    window.into_super().into_super()
}

/// Hand the session to the main queue — see this module's doc.
pub(crate) fn start(req: AuthSessionRequest, token: SessionToken) -> Result<(), AuthSessionError> {
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the actual OS main thread, so
        // constructing a `MainThreadMarker` here without re-checking is
        // sound (`plugins/haptics/src/apple.rs`'s identical claim).
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        start_on_main(mtm, req, token);
    });
    Ok(())
}

/// Build, configure and present the session. Runs on the main queue; every
/// failure path resolves the live session rather than returning, because
/// [`start`] has already told its caller `Ok(())` by the time this runs.
fn start_on_main(mtm: MainThreadMarker, req: AuthSessionRequest, token: SessionToken) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(&req.url)) else {
        // `crate::validate` already accepted this URL; `NSURL` applies its
        // own stricter parse on top. The message names neither the URL nor
        // any part of it (the crate doc's URL-never-in-a-string rule).
        crate::resolve(Err(AuthSessionError::Platform(
            "the platform URL parser rejected the request URL".to_string(),
        )));
        return;
    };

    let Some(window) = anchor_window(mtm) else {
        // No window means no presentation anchor, and a session started
        // without one fails `-start` with
        // `ASWebAuthenticationSessionErrorCodePresentationContextNotProvided`
        // anyway — refusing here reports the real reason instead. An app
        // that starts a session before its first window exists (during
        // launch, say) lands on this path.
        crate::resolve(Err(AuthSessionError::Platform(
            "no window to present the authentication session over".to_string(),
        )));
        return;
    };
    let anchor = Anchor::new(mtm, window);

    let generation = LAST_GENERATION.with(|last| {
        let next = last.get().wrapping_add(1);
        last.set(next);
        next
    });

    let block = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
        // Held for the life of this block, naming the session it completes
        // — `crate::resolve` looks the live sender up itself (see
        // `crate::SessionToken`).
        let _token: &SessionToken = &token;

        // This runs in an AuthenticationServices frame: an unwind out of
        // here is undefined behavior, not a bug
        // (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule).
        let completed = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: `url` and `error` are this completion handler's own
            // arguments — each is either null or a valid object the caller
            // keeps alive for the duration of the call.
            crate::resolve(unsafe { outcome_from(url, error) });
        }));
        if completed.is_err() {
            // A no-op if the panic happened after `resolve` already took
            // the sender (`crate::resolve`'s own documented contract).
            crate::resolve(Err(AuthSessionError::Platform(
                "panicked while completing the authentication session".to_string(),
            )));
        }

        release_live(generation);
    });

    let scheme = NSString::from_str(&req.callback_scheme);
    // The block is heap-allocated and outlives this call: it is moved into
    // `LIVE` below and released only after completion.
    let completion: ASWebAuthenticationSessionCompletionHandler =
        std::ptr::from_ref(&*block).cast_mut();
    // SAFETY: `completion` points at `block`, which lives until
    // `release_live` clears `LIVE` after the session has completed — longer
    // than the session itself can call it.
    let session = unsafe { make_session(&url, &scheme, completion) };

    // SAFETY: two property writes on a freshly built session, on the main
    // thread, before `-start`. `presentationContextProvider` is a weak
    // property, which is why `anchor` is moved into `LIVE` below rather
    // than dropped here (module doc's *Retained until completion*);
    // `prefersEphemeralWebBrowserSession` is documented to have no effect
    // after `-start`, so it is set before it.
    unsafe {
        session.setPresentationContextProvider(Some(ProtocolObject::from_ref(&*anchor)));
        session.setPrefersEphemeralWebBrowserSession(req.ephemeral);
    }

    // Set BEFORE `-start`: a session that completes synchronously (an
    // immediate refusal) would otherwise call `release_live` against an
    // empty slot and leave these objects owned by nothing.
    let previous = LIVE.with(|live| {
        let mut slot = live.borrow_mut();
        slot.replace((generation, session.clone(), anchor, block))
    });
    drop(previous);

    // SAFETY: `-start` on a fully configured session, on the main thread,
    // called exactly once for this instance.
    if !unsafe { session.start() } {
        let previous = take_live(generation);
        drop(previous);
        crate::resolve(Err(AuthSessionError::Platform(
            "the platform refused to start the authentication session".to_string(),
        )));
    }
}

/// `-[ASWebAuthenticationSession initWithURL:callbackURLScheme:completionHandler:]`.
///
/// Deprecated in favour of the iOS 17.4 `-initWithURL:callback:completionHandler:`,
/// which does not exist on this crate's iOS 15 floor — see the module doc's
/// *The iOS 15 floor and the deprecated initializer*.
///
/// # Safety
///
/// `completion` must point at a live block of the
/// `void (^)(NSURL *, NSError *)` shape, and that block must stay alive for
/// as long as the returned session can call it.
#[allow(deprecated)]
unsafe fn make_session(
    url: &NSURL,
    callback_scheme: &NSString,
    completion: ASWebAuthenticationSessionCompletionHandler,
) -> Retained<ASWebAuthenticationSession> {
    // SAFETY: a designated initializer on a fresh allocation, with a live
    // URL and scheme string; `completion`'s validity is the caller's.
    unsafe {
        ASWebAuthenticationSession::initWithURL_callbackURLScheme_completionHandler(
            ASWebAuthenticationSession::alloc(),
            url,
            Some(callback_scheme),
            completion,
        )
    }
}

/// Decode a completion-handler callback into the crate's own outcome.
///
/// A cancelled session is an [`AuthSessionOutcome::Cancelled`], not an
/// error: `ASWebAuthenticationSessionErrorCodeCanceledLogin` is what both a
/// dismissed browser tab and a declined "share login information" alert
/// report.
///
/// An error is described by its domain and code only — never its
/// `localizedDescription`, which can quote the URL the session was loading
/// (the crate doc's URL-never-in-a-string rule).
///
/// # Safety
///
/// `url` and `error` must each be either null or a valid pointer to a live
/// `NSURL`/`NSError` for the duration of the call.
unsafe fn outcome_from(
    url: *mut NSURL,
    error: *mut NSError,
) -> Result<AuthSessionOutcome, AuthSessionError> {
    if !url.is_null() {
        // SAFETY: non-null and valid, by this function's contract.
        let url = unsafe { &*url };
        return match url.absoluteString() {
            Some(absolute) => Ok(AuthSessionOutcome::Callback(absolute.to_string())),
            // Only a URL with no absolute form at all, which a redirect
            // target cannot be. Reported rather than turned into an empty
            // callback string, which a caller would parse as a real answer.
            None => Err(AuthSessionError::Platform(
                "the callback URL had no absolute form".to_string(),
            )),
        };
    }

    if !error.is_null() {
        // SAFETY: non-null and valid, by this function's contract.
        let error = unsafe { &*error };
        let domain = error.domain();
        let code = error.code();
        // SAFETY: reading a framework-owned `extern` static, valid for the
        // lifetime of the process.
        let session_domain = unsafe { ASWebAuthenticationSessionErrorDomain };
        if &*domain == session_domain
            && code == ASWebAuthenticationSessionErrorCode::CanceledLogin.0
        {
            return Ok(AuthSessionOutcome::Cancelled);
        }
        return Err(AuthSessionError::Platform(format!("{domain} {code}")));
    }

    Err(AuthSessionError::Platform(
        "the authentication session completed with neither a URL nor an error".to_string(),
    ))
}

/// Release generation `generation`'s objects on a **later** main-queue turn
/// — see the module doc's *Retained until completion* for why not inline,
/// and why the tag is checked rather than the slot blindly emptied.
fn release_live(generation: u64) {
    DispatchQueue::main().exec_async(move || {
        let previous = take_live(generation);
        drop(previous);
    });
}

/// Take the live entry out of [`LIVE`], but only if it is still generation
/// `generation`'s. Must run on the main thread.
fn take_live(generation: u64) -> Option<LiveSession> {
    LIVE.with(|live| {
        let mut slot = live.borrow_mut();
        match slot.as_ref() {
            Some(entry) if entry.0 == generation => slot.take(),
            _ => None,
        }
    })
}

/// The window to present over: the key window of a foreground-active scene,
/// falling back to any window of one, then to any window scene at all.
///
/// The framework rejects an anchor that is not in a foreground scene
/// (`ASWebAuthenticationSessionErrorCodePresentationContextInvalid`), which
/// is why an active scene is preferred over merely the first one found.
/// `None` — no window scene, or none with a window — means the app has no
/// UI to present over yet.
#[cfg(target_os = "ios")]
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();

    let mut fallback = None;
    for scene in scenes.iter() {
        let Some(window_scene) = scene.downcast_ref::<UIWindowScene>() else {
            continue;
        };
        let Some(window) = window_scene
            .keyWindow()
            .or_else(|| window_scene.windows().firstObject())
        else {
            continue;
        };

        if scene.activationState() == UISceneActivationState::ForegroundActive {
            return Some(window);
        }
        fallback = fallback.or(Some(window));
    }

    fallback
}

/// The window to present over: the key window, then the main window, then
/// any window the application owns. `None` means the app has no window to
/// present over yet.
#[cfg(target_os = "macos")]
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let app = NSApplication::sharedApplication(mtm);
    app.keyWindow()
        .or_else(|| app.mainWindow())
        .or_else(|| app.windows().firstObject())
}
