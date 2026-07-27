//! [`NativeCtx`] — the scoped call context every Apple
//! [`NativeWidget`](crate::runtime::NativeWidget) builds its views through:
//! the iOS mirror of `crate::android::ctx`'s JNI wrapper, with almost nothing
//! left in it, because UIKit needs almost nothing.
//!
//! # What it carries, and why that is all
//!
//! A [`MainThreadMarker`] — nothing else. Where the Android context has to
//! thread a live `Env`, an `Activity`/`Context` pair, a classloader cache and
//! a local-frame wrapper, the Apple arm's equivalents are all either global or
//! automatic:
//!
//! | Android needs | Apple equivalent |
//! |---|---|
//! | a live `Env` per call | the ObjC runtime is globally reachable (`objc2`) |
//! | the app classloader (`FindClass` can't see app classes) | `NSClassFromString`/`objc2`'s linked classes need no loader |
//! | `PushLocalFrame` around hierarchy loops | ARC + `Retained`'s own `Drop` |
//! | `NewGlobalRef` to retain past the call | `Retained<T>` *is* the retain |
//! | a `Context` to construct a view against | `UIView::new(mtm)` needs only main-thread proof |
//!
//! So the one thing a UIKit call genuinely cannot do without is **proof that
//! it is on the main thread**, and that is exactly what this type carries
//! (PLAN 2.1's "compile-time main-thread proof"). A control's `create`/
//! `update`/`dispose` takes `&mut NativeCtx<'_, '_>`, gets a
//! [`MainThreadMarker`] out of it, and every `objc2-ui-kit` constructor that
//! demands one is then callable — with the check done once, by the type
//! system, instead of a `debug_assert` per export the way the Android arm has
//! to.
//!
//! # No `CATransaction` batch helper here — the host already opened one
//!
//! PLAN 2.2 asks for a `CATransaction` batch (implicit animations disabled)
//! around multi-view updates. That batch **already exists, one layer up**:
//! `FrustViewHost.applyCommands` (`platform/ios/FrustEmbedding/Sources/
//! FrustEmbedding/FrustViewHost.swift`) wraps its whole per-poll command loop
//! in `CATransaction.begin()` / `CATransaction.setDisableActions(true)` /
//! `CATransaction.commit()`, and every one of this plugin's three entry points
//! (`createView`, `updateParams`, `disposeView`) is called from inside that
//! loop. A second, nested transaction opened from Rust would batch nothing the
//! host is not already batching and would cost this crate a whole new
//! dependency (`objc2-quartz-core`) for it — so this arm deliberately ships
//! none. If a future path ever calls into UIKit *outside* the host's poll (a
//! target-action handler mutating sibling views directly, say), that is the
//! moment to add one, and this comment is the reason it wasn't added sooner.
//!
//! # Frame-setting layout only
//!
//! Nothing here (or in `crate::apple::factory`) touches
//! `translatesAutoresizingMaskIntoConstraints` or any constraint API: the host
//! positions a slot's content view by assigning `frame` from the differ's rect
//! (`FrustViewHost.applyUpdate`), exactly as PLAN 2.2 specifies. A control
//! that installed constraints would fight it.
//!
//! # Hierarchy (p3-02): `addSubview`, and nothing else
//!
//! [`NativeCtx::add_child`] is this arm's whole subtree surface — the mirror
//! of `crate::android::ctx`'s JNI `addView` wrapper, and the piece that did
//! not exist here before p3-02. **The platform lays the subtree out**, not
//! frust: a component positions its children with explicit frames or a
//! `UIStackView` of its own making, and frust keeps seeing one opaque slot
//! with one rect (`research/RESEARCH-P3.md` §4).
//!
//! There is deliberately **no local-frame wrapper on this arm**: the
//! discipline `crate::android::ctx`'s `with_frame` enforces (fifty children
//! must not pin fifty-plus local references for the whole call) has no
//! analogue under ARC, where a `Retained`'s own `Drop` is the release. The
//! public `ComponentCtx::with_local_frame` (`crate::component`) still exists
//! on this arm — it simply runs its closure — so a component's `create` reads
//! the same on both platforms.

// Mirrors `crate::android::ctx`'s own module-level allow: this is the helper
// surface the six Apple controls (p2-02) and their target-action objects
// (p2-03) will build on, and this task lands the type before its consumers
// exist. The attribute goes away with those tasks rather than growing
// per-item `allow`s in the meantime.
#![allow(dead_code)]

use std::marker::PhantomData;

use objc2::MainThreadMarker;
use objc2_ui_kit::UIView;

/// The scoped call context handed to every Apple
/// [`NativeWidget`](crate::runtime::NativeWidget) method — see the module doc.
///
/// The two lifetime parameters exist to match the shape
/// `crate::runtime`'s trait signatures were written against (`NativeCtx<'local,
/// 'env>`, where the Android arm binds them to a JNI stack frame and the
/// borrow of its `Env`). The Apple arm borrows nothing — ARC owns every
/// reference a control creates — so both are phantom here rather than a second
/// spelling of the trait for one platform.
pub(crate) struct NativeCtx<'local, 'env> {
    /// Proof, checked once by the caller and then carried by the type system,
    /// that every UIKit call made through this context is on the main thread.
    mtm: MainThreadMarker,
    _local: PhantomData<&'local ()>,
    _env: PhantomData<&'env ()>,
}

impl NativeCtx<'_, '_> {
    /// A context for one create/update/dispose call, from the caller's
    /// main-thread proof.
    ///
    /// Every caller is an ObjC-entered method on
    /// `crate::apple::factory::FrustNativeControlFactory`, which is
    /// `#[thread_kind = MainThreadOnly]` — so its `MainThreadMarker` comes
    /// from `MainThreadMarker::from(self)`, i.e. from the class's own thread
    /// kind, not from a runtime check that could fail.
    pub(crate) fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            _local: PhantomData,
            _env: PhantomData,
        }
    }

    /// The main-thread proof this call carries — what a control passes to
    /// every `objc2-ui-kit` constructor, and to
    /// [`AppleHandle::view`](crate::registry::apple::AppleHandle::view).
    pub(crate) fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    /// `parent.addSubview(child)` — hierarchy building (p3-02), the Apple
    /// mirror of `crate::android::ctx`'s JNI `addView` wrapper.
    ///
    /// UIKit retains `child` for as long as it is a subview, and releases it
    /// on `removeFromSuperview` or when `parent` itself is released — so the
    /// paired-delete discipline the Android arm has to hand-hold is ARC's job
    /// here (module doc's table). Infallible: both arguments are typed,
    /// non-nil `UIView`s and the only main-thread proof needed is already
    /// carried by this context.
    pub(crate) fn add_child(&mut self, parent: &UIView, child: &UIView) {
        parent.addSubview(child);
    }
}
