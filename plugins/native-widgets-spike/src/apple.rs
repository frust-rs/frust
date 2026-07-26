//! THROWAWAY — spike 2's iOS arm: a Rust `define_class!` platform-view
//! factory, zero Swift.
//!
//! The question this answers (runbook spike 2, narrowed by camera's D2): does
//! a Rust-defined ObjC class register early enough — and survive release
//! `lto = "fat"` + `strip = "symbols"` — for the host's
//! `NSClassFromString("FrustNativeSpikeFactory")` +
//! `class_conformsToProtocol` + `init()` resolution to succeed?
//!
//! **When registration happens (the ordering the runbook demands be made
//! explicit):** objc2 registers a `define_class!` class with the ObjC runtime
//! LAZILY, on the first Rust call to `Class::class()`. Nothing on the Swift
//! side can trigger that, so the plugin exposes [`ensure_registered`] and the
//! app calls it during rebuild (any time before the first `create` command is
//! polled — the host polls AFTER the frame, so rebuild-time is always early
//! enough). Phase 2's production shape is the same call at plugin init.
//!
//! No C symbol needs to survive the link for THIS design — the class is
//! registered from live Rust code, so `Contribution::AppCrateMacro` is only
//! the *fallback's* requirement (a Swift factory calling a Rust export, the
//! camera shape). Record whichever ships.
//!
//! The Swift-side protocol (`FrustPlatformViewFactory`, an `@objc public
//! protocol` in FrustEmbedding) is looked up by name at class-registration
//! time; the app binary registers it when it loads, strictly before any Rust
//! code runs — the ordering is safe by construction.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, extern_protocol, sel};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{UIButton, UIControlEvents, UIControlState, UIView};

use std::sync::Mutex;

/// `"key":<integer>` from a flat JSON object string (spike-grade; duplicated
/// from the android arm — a shared module is not worth a throwaway's churn).
fn json_i64(s: &str, key: &str) -> Option<i64> {
    let pat = format!("\"{key}\":");
    let start = s.find(&pat)? + pat.len();
    let rest = s[start..].trim_start();
    let end = rest
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].parse().ok()
}

extern_protocol!(
    /// The FrustEmbedding Swift package's `@objc public protocol
    /// FrustPlatformViewFactory` — declared here by (runtime) name so the
    /// Rust class below can register conformance the host's
    /// `class_conformsToProtocol` check sees. Only the required method is
    /// declared; the optional ones are implemented as plain methods with the
    /// protocol's selectors.
    ///
    /// SAFETY: selector and types mirror the Swift declaration
    /// (`createView(paramsJson: String) -> UIView` ⇒
    /// `createViewWithParamsJson:` returning nonnull `UIView`).
    #[name = "FrustPlatformViewFactory"]
    pub unsafe trait FrustPlatformViewFactoryProtocol: NSObjectProtocol {
        #[unsafe(method(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView>;
    }
);

/// Last observed `"click":N` bump (the Sim-click edge trigger, mirroring the
/// Android arm's `click_bumps` — single-control spike scope on iOS).
static CLICK_BUMP: Mutex<Option<i64>> = Mutex::new(None);

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - No custom Drop; no ivars.
    #[unsafe(super(NSObject))]
    // The host resolves + instantiates the factory and calls createView on
    // the main thread (the protocol's documented contract), and UIKit types
    // are main-thread-only — so is this class.
    #[thread_kind = MainThreadOnly]
    #[name = "FrustNativeSpikeFactory"]
    struct FrustNativeSpikeFactory;

    unsafe impl NSObjectProtocol for FrustNativeSpikeFactory {}

    // SAFETY: implements the protocol's one required method with the
    // declared signature; the host calls it on the main thread.
    unsafe impl FrustPlatformViewFactoryProtocol for FrustNativeSpikeFactory {
        #[unsafe(method_id(createViewWithParamsJson:))]
        #[allow(non_snake_case)]
        fn createView_paramsJson(&self, params_json: &NSString) -> Retained<UIView> {
            let params = params_json.to_string();
            log::info!("spike-ios create params={params}");
            let mtm = MainThreadMarker::from(self);
            let button = UIButton::new(mtm);
            let title = NSString::from_str("Native (Rust-built)");
            unsafe {
                button.setTitle_forState(Some(&title), UIControlState::Normal);
                // The factory instance is the action target. UIControl holds
                // its target WEAKLY — safe here because the host's
                // factoryCache retains the factory for the app's lifetime.
                let target: &AnyObject = (self as &Self).as_ref();
                button.addTarget_action_forControlEvents(
                    Some(target),
                    sel!(spikeTapped:),
                    UIControlEvents::TouchUpInside,
                );
            }
            let control: Retained<objc2_ui_kit::UIControl> = Retained::into_super(button);
            Retained::into_super(control)
        }
    }

    impl FrustNativeSpikeFactory {
        /// Target-action landing in Rust: dispatch through the same shared
        /// handler as Android's `nativeOnEvent` (control id 1, kind 1).
        #[unsafe(method(spikeTapped:))]
        fn spike_tapped(&self, _sender: &UIButton) {
            let outcome = std::panic::catch_unwind(|| {
                log::info!("spike-ios event control=1 kind=1");
                let handler = crate::handler_slot()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if let Some(h) = handler {
                    h(1, 1);
                }
            });
            if outcome.is_err() {
                log::error!("spike-ios: panic in click handler (swallowed at FFI boundary)");
            }
        }

        /// The protocol's optional `updateParams(_:paramsJson:)` — the
        /// Sim-click edge trigger (`sendActionsForControlEvents` stands in
        /// for Android's `performClick`).
        #[unsafe(method(updateParams:paramsJson:))]
        fn update_params(&self, view: &UIView, params_json: &NSString) {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let params = params_json.to_string();
                let Some(bump) = json_i64(&params, "click") else {
                    return;
                };
                let fire = {
                    let mut last = CLICK_BUMP.lock().unwrap_or_else(|e| e.into_inner());
                    let prev = last.replace(bump);
                    prev.is_some_and(|p| p != bump)
                };
                if fire {
                    log::info!("spike-ios updateParams sim-click bump={bump}");
                    // SAFETY: this arm only ever creates UIButtons, and the
                    // host hands back the same view createView returned.
                    let button: &UIButton = unsafe { &*(view as *const UIView as *const UIButton) };
                    button.sendActionsForControlEvents(UIControlEvents::TouchUpInside);
                }
            }));
            if outcome.is_err() {
                log::error!("spike-ios: panic in updateParams (swallowed at FFI boundary)");
            }
        }

        /// The protocol's optional `disposeView(_:)` — nothing is retained
        /// per-control on this arm (the host owns the view), so this is the
        /// receipt log only.
        #[unsafe(method(disposeView:))]
        fn dispose_view(&self, _view: &UIView) {
            log::info!("spike-ios dispose");
        }
    }
);

/// Force the lazy ObjC-runtime registration of the Rust factory class and
/// report the registered name — call from app code any time before the first
/// platform-view `create` command is polled (see the module doc's ordering
/// note). Idempotent and cheap after the first call.
pub fn ensure_registered() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let class = FrustNativeSpikeFactory::class();
        // eprintln!, not log::info! — release builds carry the `lean` log
        // ceiling (`release_max_level_warn`), which compiles info logs out;
        // stderr still reaches `devicectl ... launch --console`.
        let proto = objc2::runtime::AnyProtocol::get(c"FrustPlatformViewFactory");
        eprintln!(
            "spike-ios: registered class {:?}; bare-name protocol lookup: {}; conforms: {}",
            class.name(),
            proto.is_some(),
            proto.is_some_and(|p| class.conforms_to(p)),
        );
    });
}
