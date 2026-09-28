//! [`NativeCtx`] — the scoped call context every macOS
//! [`NativeWidget`](crate::runtime::NativeWidget) builds its views through:
//! the AppKit twin of `crate::apple::ctx`, and just as small, for the same
//! reason — AppKit needs almost nothing threaded through a call.
//!
//! # What it carries, and why that is all
//!
//! A [`MainThreadMarker`] — nothing else. The table in `crate::apple::ctx`'s
//! module doc holds verbatim here: the Objective-C runtime is globally
//! reachable, ARC and `Retained<T>` replace Android's local frames and global
//! references, and `NSView::new(mtm)` needs no `Context` to construct against.
//! The one thing an AppKit call genuinely cannot do without is **proof that it
//! is on the main thread**, and that is what this type carries: every
//! `objc2-app-kit` view class is `MainThreadOnly`, so a control's
//! `create`/`update`/`dispose` gets a [`MainThreadMarker`] out of its
//! `&mut NativeCtx<'_, '_>` and every constructor that demands one is then
//! callable, checked once by the type system.
//!
//! # No `CATransaction` batch helper here either
//!
//! The iOS arm ships none because `FrustViewHost` already wraps its per-poll
//! command loop in a transaction that disables implicit animations. The
//! desktop host (`crates/frust-shell-macos`' platform-view host) opens none,
//! and this arm still adds none, because the thing that batch exists to
//! suppress does not happen here: AppKit does not implicitly animate a
//! property changed through an `NSView`/`NSControl` setter (animation needs
//! the view's `animator()` proxy or an `NSAnimationContext` that allows it),
//! and a layer-backed view is its backing layer's delegate, answering "no
//! action" for a direct layer property write outside such a context. The whole
//! host command batch (create, `setFrame:`, `setHidden:`, `update_params`)
//! runs inside one run-loop turn and is displayed together at its end. The
//! case that would change this is a control driving a sublayer of its own
//! (which has no view delegate to veto its implicit actions); that is the
//! moment to add a batch, and this comment is why it was not added sooner.
//!
//! # Frame-setting layout only
//!
//! Nothing here (or in `crate::appkit::factory`) installs an Auto Layout
//! constraint or touches `translatesAutoresizingMaskIntoConstraints`: the
//! desktop host positions a slot's view by assigning its frame from the
//! differ's rect (`setFrame:` in logical points, top-left origin — winit's
//! content view is flipped), and shrinks that frame to the slot's clip. A
//! control that installed constraints would fight it.
//!
//! # Hierarchy: `addSubview`, and nothing else
//!
//! [`NativeCtx::add_child`] is this arm's whole subtree surface — the mirror
//! of `crate::apple::ctx`'s `addSubview` wrapper, and the seam the `DemoCard`
//! composite's macOS arm and any future composite build on. **The platform
//! lays the subtree out**, not frust: a component positions its children with
//! explicit frames of its own, and frust keeps seeing one opaque slot with one
//! rect.
//!
//! There is deliberately **no local-frame wrapper on this arm**, for the same
//! ARC reason as iOS.

// Mirrors `crate::apple::ctx`'s module-level allow: `add_child` lands here
// before its first caller (the composite's macOS arm) exists, and the
// attribute goes away once it does rather than growing per-item `allow`s.
#![allow(dead_code)]

use std::marker::PhantomData;

use objc2::MainThreadMarker;
use objc2_app_kit::NSView;

/// The scoped call context handed to every macOS
/// [`NativeWidget`](crate::runtime::NativeWidget) method — see the module doc.
///
/// The two lifetime parameters exist only to match the shape
/// `crate::runtime`'s trait signatures were written against (`NativeCtx<'local,
/// 'env>`, bound to a JNI frame and `Env` borrow on Android); like the iOS arm,
/// this one borrows nothing, so both are phantom.
pub(crate) struct NativeCtx<'local, 'env> {
    /// Proof, checked once by the caller and then carried by the type system,
    /// that every AppKit call made through this context is on the main thread.
    mtm: MainThreadMarker,
    _local: PhantomData<&'local ()>,
    _env: PhantomData<&'env ()>,
}

impl NativeCtx<'_, '_> {
    /// A context for one create/update/dispose call, from the caller's
    /// main-thread proof.
    ///
    /// Every caller is `crate::appkit::factory`'s `DesktopViewFactory` impl,
    /// which obtains the marker from `MainThreadMarker::new()` (the desktop
    /// host calls a factory only on the main thread —
    /// `frust_plugin::desktop`'s contract — and the factory refuses the call
    /// rather than guessing when that check fails).
    pub(crate) fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            _local: PhantomData,
            _env: PhantomData,
        }
    }

    /// The main-thread proof this call carries — what a control passes to
    /// every `objc2-app-kit` constructor, and to
    /// [`AppKitHandle::view`](crate::registry::appkit::AppKitHandle::view).
    pub(crate) fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    /// `parent.addSubview(child)` — hierarchy building, the AppKit mirror of
    /// `crate::apple::ctx`'s `addSubview` wrapper.
    ///
    /// AppKit retains `child` for as long as it is a subview and releases it
    /// on `removeFromSuperview` or when `parent` itself is released, so the
    /// paired-delete discipline the Android arm hand-holds is ARC's job here.
    /// Infallible: both arguments are typed, non-nil `NSView`s and the only
    /// main-thread proof needed is already carried by this context.
    pub(crate) fn add_child(&mut self, parent: &NSView, child: &NSView) {
        parent.addSubview(child);
    }
}
