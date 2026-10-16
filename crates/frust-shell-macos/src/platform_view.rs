//! The macOS platform-view host: the per-OS half of the desktop
//! native-sibling compositor.
//!
//! `frust-shell-desktop` owns the OS-neutral half — the differ that turns each
//! paint's platform-view frames into an idempotent
//! create/update/dispose backlog — and hands the batch to
//! [`DesktopExtensions::on_platform_view_commands`](frust_shell_desktop::extensions::DesktopExtensions::on_platform_view_commands)
//! once a frame has been submitted. This module is what macOS does with it:
//! resolve the `view_type` string to a plugin's
//! [`DesktopViewFactory`], ask it for an `NSView`, and parent that view
//! **above** the window's content view, moving and showing/hiding it from
//! every later command.
//!
//! # Native views above the surface, no punch
//!
//! The hosted view is an opaque AppKit sibling composited *over* the frust
//! wgpu surface, not a hole cut through it: nothing here makes the window or
//! the surface translucent, and the frust slot behind the view keeps
//! painting whatever it paints (it is simply covered). That is the whole
//! z-strategy — `addSubview:positioned:relativeTo:` with
//! [`NSWindowOrderingMode::Above`] and no relative sibling, which puts each
//! new view at the top of the content view's subview list, above winit's own
//! layer-backed view content.
//!
//! # Geometry: logical points, top-left origin, no y-flip
//!
//! A command's `rect`/`clip` are **logical points in absolute window
//! coordinates**, which is exactly the unit `NSView::setFrame:` wants — no
//! scale factor is applied anywhere in this module (the hook's `scale`
//! argument exists for a host that needs physical px; AppKit does not).
//!
//! No y-flip either, and that is a property of *winit's* view rather than of
//! AppKit in general: a subview's frame is expressed in its superview's
//! coordinate system, the superview here is winit's `WinitView`, and that
//! class overrides `isFlipped` to `true` — "winit uses the upper-left corner
//! as the origin". So a rect's `(x0, y0)` is the view's frame origin as-is.
//! Were that override ever to change, every hosted view in the window would
//! appear mirrored about the window's horizontal centerline, and this is the
//! one line that would have to change.
//!
//! # Clipping: the frame *is* the clip
//!
//! An `NSView` has no clip rectangle of its own — a subview is clipped only
//! by an ancestor that clips (an `NSClipView`, or a layer with
//! `masksToBounds`), and the window's content view does neither. So a
//! command's `clip` is applied by **shrinking the frame to the intersection**
//! rather than by setting some separate clip property: a partially scrolled-
//! out view is placed at its visible sub-rectangle. The consequence is
//! deliberate and worth knowing — the native content is *resized*, not
//! masked, so a view whose content scales with its bounds (a video layer with
//! a resizing `videoGravity`, say) re-fits as it clips instead of sliding
//! under an edge. An empty intersection becomes a zero-sized frame, which
//! draws nothing.
//!
//! # Retain accounting — one retain, exchanged twice
//!
//! [`frust_plugin::desktop`]'s contract, and the matching half every factory
//! implements, is that `create` hands out a **+1-retained** view pointer and
//! `dispose` takes that same pointer back and releases it. This host holds
//! exactly that one retain for the lifetime of a slot and adds none of its
//! own:
//!
//! - **Create**: the handle's raw pointer is adopted with
//!   [`Retained::from_raw`](objc2::rc::Retained::from_raw), which *takes
//!   ownership of the existing +1* rather than adding one. The subsequent
//!   `addSubview:` makes AppKit retain the view too, but that retain is
//!   AppKit's own and is balanced by `removeFromSuperview` — it is not part
//!   of this accounting.
//! - **Update / UpdateParams**: never touch the count. `update_params` lends
//!   the factory a [`DesktopViewHandle`] built from the still-owned pointer;
//!   the handle has no `Drop`, so lending one and dropping it is a no-op.
//! - **Dispose**: `removeFromSuperview` first (releasing AppKit's retain,
//!   which cannot be the last one — this host still holds its own), then
//!   [`Retained::into_raw`](objc2::rc::Retained::into_raw) gives the +1 back
//!   to the factory through `dispose`, which releases it. Net zero.
//!
//! No path leaks: a view that cannot be hosted at all has its retain
//! released by [`NativeViewOps::attach`] before it refuses, and every view
//! that *is* hosted either stays in a slot or goes back to its factory —
//! including through [`AppKitViewHost::suspend`], which disposes every live
//! slot rather than merely hiding it.
//!
//! # Overlay surfaces (`FRUST_OVERLAYS`, default off)
//!
//! A Mode A window cannot show frust paint above a hosted view. With the
//! switch on ([`frust_shell_desktop::extensions::overlays_enabled`] — the same
//! knob the Android and iOS hosts read) the desktop core cuts each frame at its
//! slots and the batch carries `ViewCommand::Overlay`/`DisposeOverlay`. For
//! each overlay this host stacks an `NSView` subclass hosting a non-opaque
//! `CAMetalLayer`, whose `hitTest:` answers nil so every click still reaches
//! the hosted view or winit's content view under it, **directly above the
//! overlay's own slot view** (`addSubview:positioned:Above relativeTo:`), at
//! the command's rect — the whole window. A slot view hosted later is added at
//! the top, so it covers the overlays of the slots painted before it: frust
//! paint recorded between two native views stays between them. The layer is
//! published to the core's render side
//! ([`frust_shell_desktop::extensions::publish_overlay_layer`], which builds a
//! surface on it and draws the overlay's command range there), a frame is
//! requested so that happens at once, and on dispose or suspend the layer is
//! withdrawn — which waits until the render side dropped the surface — before
//! the view is removed. Off, nothing here runs.
//!
//! # Why an OS-neutral core behind a seam
//!
//! The slot bookkeeping, the warn-once miss log, the geometry mapping and the
//! whole ownership dance above are ordinary Rust with nothing AppKit about
//! them, and they are where the bugs live. They are therefore written against
//! a [`NativeViewOps`] seam and compiled on **every** host, with
//! [`AppKitOps`] — the only part that sends an Objective-C message — behind
//! `cfg(target_os = "macos")`. A fake `NativeViewOps` over counted dummy
//! pointers then exercises the accounting in an ordinary unit test, on a
//! build host with no AppKit and no window, which is the only way this logic
//! is testable at all: a `winit::Window` cannot be constructed without a live
//! event loop.
//!
//! # Main thread only
//!
//! Every [`DesktopViewFactory`] method and every `NSView` message here must
//! run on the platform main thread. The hook fires from the winit event loop,
//! which *is* that thread, so [`MainThreadMarker::new`](objc2::MainThreadMarker::new)
//! answers `Some` in practice; a `None` is treated as a bug in the caller and
//! skips the batch (logged once) rather than reaching AppKit from the wrong
//! thread.
//!
//! # No unwind into AppKit
//!
//! Factory calls are third-party plugin code invoked from inside a frame
//! callback that AppKit is on the stack of, so each one is wrapped in
//! `catch_unwind` per `docs/CODE_STANDARDS.md`'s no-panic-across-FFI rule —
//! the same discipline [`crate::appkit_glue`] applies to its observer
//! callback. A caught panic leaves a dead slot rather than tearing the app
//! down.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;

use frust_plugin::desktop::{DesktopViewFactory, DesktopViewHandle};
use frust_shell_common::platform_view::ViewCommand;
use kurbo::Rect;

/// A native view's placement, in logical points with a top-left origin —
/// what [`fit_frame`] resolves a command's `rect`/`clip` pair down to, and
/// the only geometry [`NativeViewOps`] ever sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    /// Left edge, absolute window coordinates.
    pub(crate) x: f64,
    /// Top edge, absolute window coordinates (top-left origin — see the
    /// module docs on `isFlipped`).
    pub(crate) y: f64,
    /// Width; never negative.
    pub(crate) width: f64,
    /// Height; never negative.
    pub(crate) height: f64,
}

/// Resolve a command's paint rect and optional visible-rect clip into the
/// frame the native view is given.
///
/// The clip is folded into the frame by intersection because an `NSView` has
/// no clip rect of its own (module docs' *Clipping*). A non-overlapping
/// intersection — kurbo answers those with an inverted rectangle rather than
/// an empty one — is clamped to zero size, never to a negative one, since a
/// negative `NSSize` is undefined behavior in AppKit's own geometry rather
/// than a small rectangle.
pub(crate) fn fit_frame(rect: Rect, clip: Option<Rect>) -> Frame {
    let visible = match clip {
        Some(clip) => rect.intersect(clip),
        None => rect,
    };
    Frame {
        x: visible.x0,
        y: visible.y0,
        width: (visible.x1 - visible.x0).max(0.0),
        height: (visible.y1 - visible.y0).max(0.0),
    }
}

/// The native-view operations [`ViewHost`] drives — the seam that keeps the
/// host's bookkeeping testable off macOS (module docs' *Why an OS-neutral
/// core*).
///
/// Every method is called on the platform main thread, and `attach`/`detach`
/// are the two ownership-transfer points: `attach` takes the factory's +1,
/// `detach` gives it back.
pub(crate) trait NativeViewOps {
    /// One attached native view, as the host holds it while its slot lives —
    /// `Retained<NSView>` on macOS, a counted test double under test.
    type View;

    /// Resolve the factory registered for `view_type`, or `None` when
    /// nothing has registered that string.
    fn lookup_factory(&self, view_type: &str) -> Option<Arc<dyn DesktopViewFactory>>;

    /// Adopt a factory's +1-retained view pointer (taking over that retain,
    /// adding none) and parent it above the window's existing content.
    ///
    /// `None` means the view could not be hosted — a factory contract
    /// violation (a null pointer), or no view hierarchy left to parent into.
    /// An implementation must have released whatever retain the pointer
    /// carried before answering `None`, since nothing else can: the handle
    /// was already consumed to get here.
    fn attach(&self, view: NonNull<c_void>) -> Option<Self::View>;

    /// The attached view's raw pointer, borrowed: no ownership moves and the
    /// retain count is untouched.
    fn raw(&self, view: &Self::View) -> NonNull<c_void>;

    /// Place the view at `frame` (logical points, top-left origin).
    fn place(&self, view: &Self::View, frame: Frame);

    /// Show or hide the view without disposing it.
    fn set_hidden(&self, view: &Self::View, hidden: bool);

    /// Unparent the view and hand its +1 back, for return to the factory.
    fn detach(&self, view: Self::View) -> NonNull<c_void>;

    /// One overlay view, as the host holds it while it is stacked —
    /// the view plus its `CAMetalLayer` on macOS.
    type Overlay;

    /// Create an overlay view (input-transparent, hosting a non-opaque
    /// `CAMetalLayer`) placed at `frame`, directly above `above` — its slot's
    /// view — in the content view's subviews. `None` when it cannot be built —
    /// no content view, or no `CAMetalLayer` class.
    fn attach_overlay(&self, above: &Self::View, frame: Frame) -> Option<Self::Overlay>;

    /// Move and resize an overlay view to `frame`.
    fn place_overlay(&self, overlay: &Self::Overlay, frame: Frame);

    /// The overlay's `CAMetalLayer*`, borrowed (the overlay keeps it alive),
    /// and the drawable size its current frame needs, physical px.
    fn overlay_layer(&self, overlay: &Self::Overlay) -> (NonNull<c_void>, (u32, u32));

    /// Hand the render side overlay `overlay_id`'s layer at `size` physical
    /// px; answers whether something changed (a frame is then owed).
    fn publish_overlay(&self, overlay_id: u64, layer: NonNull<c_void>, size: (u32, u32)) -> bool;

    /// Take overlay `overlay_id`'s layer back from the render side; returns
    /// once the surface built on it is dropped.
    fn withdraw_overlay(&self, overlay_id: u64);

    /// Ask for a frame now — a newly published or resized overlay layer is
    /// drawn on the next frame, which an idle window would otherwise not
    /// produce.
    fn request_frame(&self);

    /// Unparent and release an overlay view. The surface built on its layer
    /// must already be gone ([`Self::withdraw_overlay`]).
    fn detach_overlay(&self, overlay: Self::Overlay);
}

/// One live slot: the native view this host owns a retain of, and the
/// factory that owes it a `dispose`.
///
/// The factory is held per-slot rather than looked up again at dispose time
/// so that a view is always returned to the *same* factory that made it,
/// even if the registry were to change underneath (it cannot today — first
/// registration wins — but the alternative would silently hand one plugin's
/// view to another's `dispose`).
struct Slot<V> {
    view: V,
    factory: Arc<dyn DesktopViewFactory>,
}

/// One stacked overlay: its view, the slot it sits above, and the frame it was
/// last placed at.
struct OverlayView<O> {
    overlay: O,
    frame: Frame,
}

/// The OS-neutral platform-view host: live slots and overlays, plus the set of
/// `view_type`s already reported as unresolvable.
///
/// Idempotent by construction, as the hook's contract requires: a `Create`
/// for a slot that already exists is ignored, an `Update`/`UpdateParams` for
/// an unknown slot is ignored, and a `Dispose` for a slot already gone is a
/// no-op. That matters because a missed batch is re-served whole, so an
/// already-applied prefix can arrive a second time.
pub(crate) struct ViewHost<O: NativeViewOps> {
    slots: HashMap<u64, Slot<O::View>>,
    missing: HashSet<String>,
    /// Whether this host builds overlay views for the `Overlay` commands
    /// (module docs' *Overlay surfaces*).
    overlays_enabled: bool,
    /// The stacked overlays by the differ's overlay id.
    overlays: HashMap<u64, OverlayView<O::Overlay>>,
    /// Overlays that could not be stacked (no slot view, or no view could be
    /// built), remembered so each is logged once.
    unstacked: HashSet<u64>,
}

impl<O: NativeViewOps> Default for ViewHost<O> {
    fn default() -> Self {
        Self::with_overlays(frust_shell_desktop::extensions::overlays_enabled())
    }
}

impl<O: NativeViewOps> ViewHost<O> {
    /// A host that builds overlay views (`overlays_enabled`) or ignores the
    /// overlay commands.
    pub(crate) fn with_overlays(overlays_enabled: bool) -> Self {
        Self {
            slots: HashMap::new(),
            missing: HashSet::new(),
            overlays_enabled,
            overlays: HashMap::new(),
            unstacked: HashSet::new(),
        }
    }

    /// Apply one command batch in order.
    pub(crate) fn apply(&mut self, ops: &O, commands: &[ViewCommand]) {
        for command in commands {
            match command {
                ViewCommand::Create {
                    slot_id,
                    view_type,
                    params_json,
                    // Input forwarding is a native-sibling hit-testing
                    // concern this host does not implement: the view is
                    // parented above the surface and AppKit routes events to
                    // it through the ordinary responder chain, so there is
                    // nothing to switch on here.
                    interactive: _,
                } => self.create(ops, *slot_id, view_type, params_json),
                ViewCommand::Update {
                    slot_id,
                    rect,
                    clip,
                    visible,
                    // Z-shields describe regions where frust content over an
                    // interactive slot must keep winning input — meaningful
                    // only to a host that intercepts events before the native
                    // view, which this one does not.
                    shields: _,
                    // Stacking among slots is creation order (a new view goes
                    // to the top), not the rank.
                    z_index: _,
                } => self.update(ops, *slot_id, *rect, *clip, *visible),
                ViewCommand::UpdateParams {
                    slot_id,
                    params_json,
                } => self.update_params(ops, *slot_id, params_json),
                ViewCommand::Dispose { slot_id } => self.dispose(ops, *slot_id),
                ViewCommand::Overlay {
                    overlay_id,
                    above_slot_id,
                    rect,
                    // Informational: the render side draws each frame with the
                    // ranges cut from that frame's own scene.
                    command_range: _,
                } => self.overlay(ops, *overlay_id, *above_slot_id, *rect),
                ViewCommand::DisposeOverlay { overlay_id } => {
                    self.dispose_overlay(ops, *overlay_id);
                }
            }
        }
    }

    /// Create or re-place overlay `overlay_id`: a new one is stacked directly
    /// above its slot's view at `rect` and its layer published; a known one
    /// adopts a changed rect, re-publishing the layer at its new drawable
    /// size. Either change asks for a frame. Ignored with overlays off; an
    /// overlay over a slot this host has no view for (a dead slot) is skipped
    /// — its content stays on the main surface.
    fn overlay(&mut self, ops: &O, overlay_id: u64, above_slot_id: u64, rect: Rect) {
        if !self.overlays_enabled {
            return;
        }
        let frame = fit_frame(rect, None);
        if let Some(stacked) = self.overlays.get_mut(&overlay_id) {
            if stacked.frame == frame {
                return;
            }
            ops.place_overlay(&stacked.overlay, frame);
            stacked.frame = frame;
            let (layer, size) = ops.overlay_layer(&stacked.overlay);
            if ops.publish_overlay(overlay_id, layer, size) {
                ops.request_frame();
            }
            return;
        }
        let Some(slot) = self.slots.get(&above_slot_id) else {
            if self.unstacked.insert(overlay_id) {
                log::warn!(
                    "frust-overlay: overlay {overlay_id} has no hosted view for slot \
                     {above_slot_id} to sit above; its content stays under the native views"
                );
            }
            return;
        };
        let Some(overlay) = ops.attach_overlay(&slot.view, frame) else {
            if self.unstacked.insert(overlay_id) {
                log::warn!(
                    "frust-overlay: overlay view {overlay_id} could not be built; frust paint \
                     stays under the hosted views"
                );
            }
            return;
        };
        let (layer, (width, height)) = ops.overlay_layer(&overlay);
        log::info!(
            "frust-overlay: overlay {overlay_id} view stacked above slot {above_slot_id} \
             (CAMetalLayer {layer:p}, {width}x{height} px)"
        );
        self.overlays
            .insert(overlay_id, OverlayView { overlay, frame });
        if ops.publish_overlay(overlay_id, layer, (width, height)) {
            ops.request_frame();
        }
    }

    /// Tear overlay `overlay_id` down: withdraw its layer from the render side
    /// (which drops the surface on it) **before** removing the view.
    fn dispose_overlay(&mut self, ops: &O, overlay_id: u64) {
        self.unstacked.remove(&overlay_id);
        let Some(stacked) = self.overlays.remove(&overlay_id) else {
            return;
        };
        ops.withdraw_overlay(overlay_id);
        ops.detach_overlay(stacked.overlay);
        log::info!("frust-overlay: overlay {overlay_id} view removed");
    }

    /// Every stacked overlay's id, `CAMetalLayer*` and drawable size in
    /// physical px, ascending id.
    #[cfg(test)]
    pub(crate) fn overlay_layers(&self, ops: &O) -> Vec<(u64, NonNull<c_void>, (u32, u32))> {
        let mut layers: Vec<_> = self
            .overlays
            .iter()
            .map(|(&overlay_id, stacked)| {
                let (layer, size) = ops.overlay_layer(&stacked.overlay);
                (overlay_id, layer, size)
            })
            .collect();
        layers.sort_by_key(|(overlay_id, _, _)| *overlay_id);
        layers
    }

    /// Dispose every live slot, returning each view to its factory.
    ///
    /// Called when the window or its surface goes away: the hierarchy these
    /// views were parented into is being torn down, so merely hiding them
    /// would leak. Nothing is retained across the gap — the next batch after
    /// a surface comes back is a full `Create` + `Update` replay of every
    /// live slot.
    ///
    /// The warn-once miss log deliberately survives: a `view_type` that had
    /// no factory before the window went away still has none after it, and
    /// re-warning on every surface recreation would turn a one-line
    /// diagnostic into a per-event repeat.
    pub(crate) fn suspend(&mut self, ops: &O) {
        // Overlays first: each layer is withdrawn (its surface dropped) before
        // its view goes, and before the slot view under it.
        let overlay_ids: Vec<u64> = self.overlays.keys().copied().collect();
        for overlay_id in overlay_ids {
            self.dispose_overlay(ops, overlay_id);
        }
        self.unstacked.clear();
        let slot_ids: Vec<u64> = self.slots.keys().copied().collect();
        for slot_id in slot_ids {
            self.dispose(ops, slot_id);
        }
    }

    /// How many slots are live — diagnostics and tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn slot_count(&self) -> usize {
        self.slots.len()
    }

    fn create(&mut self, ops: &O, slot_id: u64, view_type: &str, params_json: &str) {
        if self.slots.contains_key(&slot_id) {
            // A re-served batch replaying a create this host already applied.
            return;
        }

        let Some(factory) = ops.lookup_factory(view_type) else {
            if self.missing.insert(view_type.to_string()) {
                log::warn!(
                    "frust-shell-macos: no desktop view factory registered for view_type \
                     {view_type:?} — the slot stays empty (the plugin owning that view type must \
                     register its factory before the view is painted)"
                );
            }
            return;
        };

        let created = catch_unwind(AssertUnwindSafe(|| factory.create(params_json)));
        let handle = match created {
            Ok(Some(handle)) => handle,
            Ok(None) => {
                log::debug!(
                    "frust-shell-macos: the {view_type:?} factory declined to create a view — \
                     slot {slot_id} stays empty"
                );
                return;
            }
            Err(_) => {
                log::error!(
                    "frust-shell-macos: panic caught in the {view_type:?} factory's create — \
                     slot {slot_id} stays empty"
                );
                return;
            }
        };

        let Some(view) = ops.attach(handle.into_raw()) else {
            log::error!(
                "frust-shell-macos: the {view_type:?} factory returned a view pointer this host \
                 could not adopt — slot {slot_id} stays empty"
            );
            return;
        };

        // Hidden until the `Update` that always follows a `Create` in the
        // same batch places it: a view added at its default zero frame and
        // shown immediately would flash at the window's origin.
        ops.set_hidden(&view, true);
        self.slots.insert(slot_id, Slot { view, factory });
    }

    fn update(&mut self, ops: &O, slot_id: u64, rect: Rect, clip: Option<Rect>, visible: bool) {
        let Some(slot) = self.slots.get(&slot_id) else {
            // An unknown slot is an update for a dead or already-disposed
            // one, never an implicit create.
            return;
        };
        ops.place(&slot.view, fit_frame(rect, clip));
        ops.set_hidden(&slot.view, !visible);
    }

    fn update_params(&mut self, ops: &O, slot_id: u64, params_json: &str) {
        let Some(slot) = self.slots.get(&slot_id) else {
            return;
        };
        // SAFETY: the pointer is the +1-retained view this host still owns
        // (it is alive in the slot and no `dispose` has consumed it), touched
        // only on the platform main thread. The handle is a lend, not a
        // transfer: `DesktopViewHandle` has no `Drop`, so building one and
        // dropping it after the call changes no retain count.
        let handle = unsafe { DesktopViewHandle::from_raw(ops.raw(&slot.view)) };
        let applied = catch_unwind(AssertUnwindSafe(|| {
            slot.factory.update_params(&handle, params_json);
        }));
        if applied.is_err() {
            log::error!(
                "frust-shell-macos: panic caught in a factory's update_params — slot {slot_id} \
                 keeps its previous params"
            );
        }
    }

    fn dispose(&mut self, ops: &O, slot_id: u64) {
        let Some(slot) = self.slots.remove(&slot_id) else {
            // Already gone: a re-served batch, or a suspend that disposed it.
            return;
        };
        let raw = ops.detach(slot.view);
        // SAFETY: `raw` is exactly the +1-retained pointer this host adopted
        // at create time, now unparented and owned by nothing else; handing
        // it to `dispose` transfers that one retain back to the factory that
        // issued it, on the platform main thread.
        let handle = unsafe { DesktopViewHandle::from_raw(raw) };
        let disposed = catch_unwind(AssertUnwindSafe(|| slot.factory.dispose(handle)));
        if disposed.is_err() {
            log::error!(
                "frust-shell-macos: panic caught in a factory's dispose — slot {slot_id}'s native \
                 view may leak"
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use appkit::AppKitViewHost;

#[cfg(target_os = "macos")]
mod appkit {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Arc;

    use frust_plugin::desktop::DesktopViewFactory;
    use frust_shell_common::platform_view::ViewCommand;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSResponder, NSView, NSWindowOrderingMode};
    use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{Frame, NativeViewOps, ViewHost};

    define_class!(
        // SAFETY:
        // - `NSView` imposes no subclassing requirement beyond main-thread
        //   use, which `MainThreadOnly` enforces.
        // - `FrustOverlaySurfaceView` holds no ivars and implements no
        //   `Drop`.
        // - The two overrides keep `NSView`'s own signatures (`hitTest:`
        //   takes an `NSPoint` and answers an `NSView *`; `isOpaque` a
        //   `BOOL`).
        #[unsafe(super(NSView, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        struct FrustOverlaySurfaceView;

        impl FrustOverlaySurfaceView {
            /// Input-transparent: no point is ever inside the overlay, so a
            /// click falls through to the hosted view or winit's content
            /// view under it, exactly as without the overlay.
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> *mut NSView {
                std::ptr::null_mut()
            }

            /// Never opaque: AppKit must composite what lies under it.
            #[unsafe(method(isOpaque))]
            fn is_opaque(&self) -> bool {
                false
            }
        }
    );

    /// One stacked overlay: the view, and the `CAMetalLayer` it hosts (also
    /// retained by the view; held here so its pointer is stable to hand out —
    /// and alive until the host has withdrawn it from the render side).
    pub(crate) struct AppKitOverlay {
        view: Retained<NSView>,
        layer: Retained<AnyObject>,
    }

    /// Build an overlay view at `frame` (logical points) hosting a non-opaque
    /// `CAMetalLayer` at `scale`, or `None` when the runtime has no
    /// `CAMetalLayer` class.
    fn overlay_view(mtm: MainThreadMarker, frame: NSRect, scale: f64) -> Option<AppKitOverlay> {
        let class = AnyClass::get(c"CAMetalLayer")?;
        // SAFETY: `+[CAMetalLayer layer]` is the class's autoreleased
        // convenience constructor (no arguments; `msg_send!` retains the
        // `none`-family result into the `Retained`).
        let layer: Retained<AnyObject> = unsafe { msg_send![class, layer] };
        // SAFETY: `setOpaque:` (`BOOL`) and `setContentsScale:` (`CGFloat`)
        // are `CALayer` setters, sent on the main thread to a layer no other
        // code holds yet.
        unsafe {
            let _: () = msg_send![&*layer, setOpaque: false];
            let _: () = msg_send![&*layer, setContentsScale: scale];
        }
        let allocated = FrustOverlaySurfaceView::alloc(mtm).set_ivars(());
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer, sent
        // once to a freshly allocated instance.
        let view: Retained<FrustOverlaySurfaceView> =
            unsafe { msg_send![super(allocated), initWithFrame: frame] };
        let view = Retained::into_super(view);
        // A layer-hosting view: the layer is set before `wantsLayer`, so
        // AppKit hosts it rather than replacing it with a backing layer of its
        // own.
        // SAFETY: `setLayer:` takes a `CALayer *`, and `layer` is one (a
        // `CAMetalLayer`); the view retains it.
        unsafe {
            let _: () = msg_send![&*view, setLayer: &*layer];
        }
        view.setWantsLayer(true);
        // The overlay covers the whole window (the differ's rect): it follows
        // the content view's live resizes between the re-placements each
        // changed rect brings.
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        Some(AppKitOverlay { view, layer })
    }

    /// `frame` as an `NSRect` — logical points, top-left origin (the content
    /// view is flipped; module docs' *Geometry*).
    fn ns_rect(frame: Frame) -> NSRect {
        NSRect::new(
            NSPoint::new(frame.x, frame.y),
            NSSize::new(frame.width, frame.height),
        )
    }

    /// The macOS platform-view host: the OS-neutral [`ViewHost`] plus the
    /// once-only diagnostics for the two ways a batch can be skipped whole.
    #[derive(Default)]
    pub(crate) struct AppKitViewHost {
        host: ViewHost<AppKitOps>,
        /// Whether the off-main-thread refusal has already been logged, so a
        /// per-frame hook cannot turn one bug into a log flood.
        off_main_thread_logged: bool,
        /// Likewise for a window whose AppKit content view cannot be
        /// resolved.
        no_content_view_logged: bool,
    }

    impl AppKitViewHost {
        /// Apply one command batch against `window`'s content view.
        pub(crate) fn apply(&mut self, window: &Arc<Window>, commands: &[ViewCommand]) {
            let Some(mtm) = self.main_thread() else {
                return;
            };
            let Some(content) = content_view(window, mtm) else {
                if !self.no_content_view_logged {
                    self.no_content_view_logged = true;
                    log::error!(
                        "frust-shell-macos: the window exposes no AppKit view handle — hosted \
                         native views cannot be placed in this window"
                    );
                }
                return;
            };
            self.host.apply(
                &AppKitOps {
                    content: Some(content),
                    mtm,
                    scale: window.scale_factor(),
                    window: Some(Arc::clone(window)),
                },
                commands,
            );
        }

        /// Remove and dispose every hosted view.
        ///
        /// Takes no window on purpose: this fires *because* the window or its
        /// surface is going away, and unparenting a view needs only the view
        /// itself (`removeFromSuperview` walks up, it is not sent to the
        /// superview). So the content view is deliberately left unresolved
        /// here — see [`AppKitOps::content`].
        pub(crate) fn suspend(&mut self) {
            let Some(mtm) = self.main_thread() else {
                return;
            };
            self.host.suspend(&AppKitOps {
                content: None,
                mtm,
                scale: 1.0,
                window: None,
            });
        }

        /// How many native views this host currently has parented — for the
        /// crate's `Debug` rendering.
        pub(crate) fn hosted_count(&self) -> usize {
            self.host.slot_count()
        }

        /// The main-thread proof every AppKit and factory call below needs,
        /// or `None` (reported once) when this hook somehow ran elsewhere.
        fn main_thread(&mut self) -> Option<MainThreadMarker> {
            let marker = MainThreadMarker::new();
            if marker.is_none() && !self.off_main_thread_logged {
                self.off_main_thread_logged = true;
                log::error!(
                    "frust-shell-macos: platform-view work arrived off the main thread — skipping \
                     it (AppKit and every view factory are main-thread only)"
                );
            }
            marker
        }
    }

    /// The window's AppKit content view — winit's own `WinitView`, which it
    /// installs with `setContentView:` and which is what its raw-window-handle
    /// reports.
    ///
    /// `_mtm` is taken to make the main-thread requirement part of the
    /// signature rather than a comment; the calls below need no marker value
    /// of their own.
    fn content_view(window: &Arc<Window>, _mtm: MainThreadMarker) -> Option<Retained<NSView>> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: `ns_view` is the live `NSView` winit owns as this window's
        // content view, borrowed for as long as the window is alive (it is,
        // this host holds an `Arc<Window>` over the whole call).
        // `Retained::retain` adds a retain of our own for the duration of the
        // batch rather than stealing winit's, which is exactly the +1 this
        // temporary needs.
        unsafe { Retained::retain(appkit.ns_view.cast::<NSView>().as_ptr()) }
    }

    /// The real AppKit implementation of the host's view operations.
    pub(crate) struct AppKitOps {
        /// The content view a newly created view is parented into, when
        /// there is one to parent into.
        ///
        /// `None` on the teardown path, where the window is going away and
        /// no view will be created — every other operation (place, hide,
        /// unparent) is sent to the hosted view itself and needs no
        /// superview handle.
        content: Option<Retained<NSView>>,
        /// The main-thread proof the overlay views' allocation needs.
        mtm: MainThreadMarker,
        /// The window's backing scale, for the overlay layers' contents
        /// scale and drawable size (unused on the teardown path).
        scale: f64,
        /// The window a frame is requested from when an overlay layer is
        /// published or resized (`None` on the teardown path, which publishes
        /// nothing).
        window: Option<Arc<Window>>,
    }

    impl NativeViewOps for AppKitOps {
        type View = Retained<NSView>;
        type Overlay = AppKitOverlay;

        fn lookup_factory(&self, view_type: &str) -> Option<Arc<dyn DesktopViewFactory>> {
            frust_plugin::desktop::lookup_view_factory(view_type)
        }

        fn attach(&self, view: NonNull<c_void>) -> Option<Retained<NSView>> {
            // SAFETY: the factory's contract is that this pointer is a
            // +1-retained `NSView` valid until it is handed back to
            // `dispose`. `from_raw` adopts that existing retain rather than
            // adding one (the module docs' retain accounting); it answers
            // `None` only for a null pointer, which carries no retain to
            // release.
            let view = unsafe { Retained::from_raw(view.cast::<NSView>().as_ptr()) }?;
            // Adopting before this check is what keeps the refusal below
            // leak-free: dropping `view` releases the retain rather than
            // stranding it. (Unreachable in practice — the only caller with
            // no content view is the teardown path, which issues no creates.)
            let content = self.content.as_ref()?;
            // `Above` with no relative sibling puts the view at the top of
            // the subview order — over winit's own content, and over any
            // hosted view added before it.
            content.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, None);
            Some(view)
        }

        fn raw(&self, view: &Retained<NSView>) -> NonNull<c_void> {
            NonNull::from(&**view).cast()
        }

        fn place(&self, view: &Retained<NSView>, frame: Frame) {
            // Logical points straight through, top-left origin — winit's
            // content view is flipped (module docs' *Geometry*).
            view.setFrame(NSRect::new(
                NSPoint::new(frame.x, frame.y),
                NSSize::new(frame.width, frame.height),
            ));
        }

        fn set_hidden(&self, view: &Retained<NSView>, hidden: bool) {
            view.setHidden(hidden);
        }

        fn detach(&self, view: Retained<NSView>) -> NonNull<c_void> {
            // Releases AppKit's own superview retain, never the host's.
            view.removeFromSuperview();
            // The +1 this host adopted at create time, handed back out for
            // return to the factory.
            NonNull::new(Retained::into_raw(view))
                .expect("Retained::into_raw never answers null")
                .cast()
        }

        fn attach_overlay(&self, above: &Retained<NSView>, frame: Frame) -> Option<AppKitOverlay> {
            let content = self.content.as_ref()?;
            let overlay = overlay_view(self.mtm, ns_rect(frame), self.scale)?;
            // Directly above its slot's view, not at the top: a slot view
            // hosted after this one (painted later) stays above it.
            content.addSubview_positioned_relativeTo(
                &overlay.view,
                NSWindowOrderingMode::Above,
                Some(above),
            );
            Some(overlay)
        }

        fn place_overlay(&self, overlay: &AppKitOverlay, frame: Frame) {
            overlay.view.setFrame(ns_rect(frame));
        }

        fn overlay_layer(&self, overlay: &AppKitOverlay) -> (NonNull<c_void>, (u32, u32)) {
            let size = overlay.view.frame().size;
            let pixels = (
                (size.width * self.scale).round().max(1.0) as u32,
                (size.height * self.scale).round().max(1.0) as u32,
            );
            (NonNull::from(&*overlay.layer).cast(), pixels)
        }

        fn publish_overlay(
            &self,
            overlay_id: u64,
            layer: NonNull<c_void>,
            (width, height): (u32, u32),
        ) -> bool {
            frust_shell_desktop::extensions::publish_overlay_layer(overlay_id, layer, width, height)
        }

        fn withdraw_overlay(&self, overlay_id: u64) {
            frust_shell_desktop::extensions::withdraw_overlay_layer(overlay_id);
        }

        fn request_frame(&self) {
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
        }

        fn detach_overlay(&self, overlay: AppKitOverlay) {
            overlay.view.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(x0, y0, x1, y1)
    }

    // --- Geometry ------------------------------------------------------

    #[test]
    fn an_unclipped_rect_maps_straight_through_in_logical_points() {
        // No scale, no y-flip: the frame is the rect. A y-flipping host would
        // answer a different `y` for a rect near the window's bottom edge.
        let frame = fit_frame(rect(12.0, 40.0, 212.0, 160.0), None);
        assert_eq!(
            frame,
            Frame {
                x: 12.0,
                y: 40.0,
                width: 200.0,
                height: 120.0
            }
        );
    }

    #[test]
    fn a_clip_shrinks_the_frame_to_the_visible_intersection() {
        // The view is scrolled half out of a viewport that starts at y=100:
        // AppKit has no clip rect, so the frame itself becomes the visible
        // half (module docs' *Clipping*).
        let frame = fit_frame(
            rect(0.0, 50.0, 100.0, 150.0),
            Some(rect(0.0, 100.0, 100.0, 400.0)),
        );
        assert_eq!(
            frame,
            Frame {
                x: 0.0,
                y: 100.0,
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn a_non_overlapping_clip_becomes_a_zero_sized_frame_never_a_negative_one() {
        // kurbo answers a disjoint intersection with an inverted rectangle;
        // handing its raw extents to AppKit would be a negative `NSSize`.
        let frame = fit_frame(
            rect(0.0, 0.0, 50.0, 50.0),
            Some(rect(200.0, 200.0, 300.0, 300.0)),
        );
        assert_eq!(frame.width, 0.0);
        assert_eq!(frame.height, 0.0);
    }

    #[test]
    fn a_clip_larger_than_the_rect_leaves_the_rect_alone() {
        let bounds = rect(10.0, 10.0, 60.0, 60.0);
        assert_eq!(
            fit_frame(bounds, Some(rect(0.0, 0.0, 1000.0, 1000.0))),
            fit_frame(bounds, None)
        );
    }

    // --- The fake native-view backend ----------------------------------

    /// Per-pointer retain bookkeeping shared by the fake factory and the
    /// fake ops, standing in for the Objective-C runtime's own count.
    #[derive(Default)]
    struct RetainLedger {
        /// Live retain count per fake view pointer address.
        counts: HashMap<usize, i32>,
        /// Addresses currently parented into the fake content view.
        attached: Vec<usize>,
        /// Hidden flag per address, `None` before the first call.
        hidden: HashMap<usize, bool>,
        /// Last frame each address was placed at.
        frames: HashMap<usize, Frame>,
        /// Params each address has been handed since creation.
        params: HashMap<usize, Vec<String>>,
        /// Next address to hand out — never null, never dereferenced.
        next_address: usize,
        /// The render-side seam's calls, in order: `publish N WxH`,
        /// `withdraw N`, `frame`, `detach N`.
        overlay_events: Vec<String>,
    }

    impl RetainLedger {
        fn new() -> Rc<RefCell<Self>> {
            Rc::new(RefCell::new(Self {
                next_address: 0x1000,
                ..Self::default()
            }))
        }

        /// Mint a fresh +1-retained fake view.
        fn create(&mut self) -> NonNull<c_void> {
            self.next_address += 0x10;
            let address = self.next_address;
            self.counts.insert(address, 1);
            self.params.insert(address, Vec::new());
            NonNull::new(std::ptr::without_provenance_mut(address))
                .expect("a nonzero address is never null")
        }

        fn release(&mut self, address: usize) {
            let count = self.counts.entry(address).or_insert(0);
            *count -= 1;
        }

        fn live(&self) -> Vec<usize> {
            let mut live: Vec<usize> = self
                .counts
                .iter()
                .filter(|(_, count)| **count > 0)
                .map(|(address, _)| *address)
                .collect();
            live.sort_unstable();
            live
        }

        fn over_released(&self) -> Vec<usize> {
            let mut bad: Vec<usize> = self
                .counts
                .iter()
                .filter(|(_, count)| **count < 0)
                .map(|(address, _)| *address)
                .collect();
            bad.sort_unstable();
            bad
        }
    }

    fn address(ptr: NonNull<c_void>) -> usize {
        ptr.as_ptr() as usize
    }

    /// A factory over the ledger: `create` mints a +1, `dispose` releases
    /// exactly that +1, `update_params` never touches the count — the same
    /// contract every real factory implements.
    struct FakeFactory {
        ledger: Rc<RefCell<RetainLedger>>,
        create_calls: AtomicUsize,
        dispose_calls: AtomicUsize,
        /// When set, `create` answers `None` (a dead slot).
        declines: bool,
        /// When set, `create` panics across the boundary.
        panics: bool,
    }

    impl FakeFactory {
        fn new(ledger: &Rc<RefCell<RetainLedger>>) -> Arc<Self> {
            Arc::new(Self {
                ledger: Rc::clone(ledger),
                create_calls: AtomicUsize::new(0),
                dispose_calls: AtomicUsize::new(0),
                declines: false,
                panics: false,
            })
        }

        fn declining(ledger: &Rc<RefCell<RetainLedger>>) -> Arc<Self> {
            Arc::new(Self {
                ledger: Rc::clone(ledger),
                create_calls: AtomicUsize::new(0),
                dispose_calls: AtomicUsize::new(0),
                declines: true,
                panics: false,
            })
        }

        fn panicking(ledger: &Rc<RefCell<RetainLedger>>) -> Arc<Self> {
            Arc::new(Self {
                ledger: Rc::clone(ledger),
                create_calls: AtomicUsize::new(0),
                dispose_calls: AtomicUsize::new(0),
                declines: false,
                panics: true,
            })
        }
    }

    // SAFETY (test-only): `DesktopViewFactory` requires `Send + Sync`, and
    // this fake holds an `Rc<RefCell<..>>` ledger that is neither. Every test
    // here drives the host from a single thread and never clones the `Arc`
    // across one, so the shared ledger is only ever touched from that thread.
    unsafe impl Send for FakeFactory {}
    // SAFETY: as above — single-threaded use only, asserted by construction
    // in this module's tests.
    unsafe impl Sync for FakeFactory {}

    impl DesktopViewFactory for FakeFactory {
        fn create(&self, params_json: &str) -> Option<DesktopViewHandle> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            assert!(!self.panics, "panicking fake factory");
            if self.declines {
                return None;
            }
            let ptr = self.ledger.borrow_mut().create();
            self.ledger
                .borrow_mut()
                .params
                .entry(address(ptr))
                .or_default()
                .push(params_json.to_string());
            // SAFETY: a freshly minted, +1-counted fake pointer that this
            // module's fake ops never dereference.
            Some(unsafe { DesktopViewHandle::from_raw(ptr) })
        }

        fn update_params(&self, view: &DesktopViewHandle, params_json: &str) {
            let ptr = NonNull::new(view.as_ptr()).expect("a handle never wraps null");
            self.ledger
                .borrow_mut()
                .params
                .entry(address(ptr))
                .or_default()
                .push(params_json.to_string());
        }

        fn dispose(&self, view: DesktopViewHandle) {
            self.dispose_calls.fetch_add(1, Ordering::SeqCst);
            let ptr = view.into_raw();
            self.ledger.borrow_mut().release(address(ptr));
        }
    }

    /// A [`NativeViewOps`] over the ledger: no AppKit, no window, and the
    /// same ownership transfers the real one performs.
    struct FakeOps {
        ledger: Rc<RefCell<RetainLedger>>,
        factories: HashMap<String, Arc<dyn DesktopViewFactory>>,
        /// When set, `attach` refuses the pointer (the null-view path).
        refuse_attach: bool,
        /// When set, `attach_overlay` cannot build an overlay.
        refuse_overlay: bool,
        /// The backing scale the fake reports drawable sizes at.
        scale: f64,
    }

    impl FakeOps {
        fn new(ledger: &Rc<RefCell<RetainLedger>>) -> Self {
            Self {
                ledger: Rc::clone(ledger),
                factories: HashMap::new(),
                refuse_attach: false,
                refuse_overlay: false,
                scale: 2.0,
            }
        }

        fn with(mut self, view_type: &str, factory: Arc<dyn DesktopViewFactory>) -> Self {
            self.factories.insert(view_type.to_string(), factory);
            self
        }
    }

    impl NativeViewOps for FakeOps {
        type View = NonNull<c_void>;

        fn lookup_factory(&self, view_type: &str) -> Option<Arc<dyn DesktopViewFactory>> {
            self.factories.get(view_type).cloned()
        }

        fn attach(&self, view: NonNull<c_void>) -> Option<NonNull<c_void>> {
            if self.refuse_attach {
                return None;
            }
            self.ledger.borrow_mut().attached.push(address(view));
            Some(view)
        }

        fn raw(&self, view: &NonNull<c_void>) -> NonNull<c_void> {
            *view
        }

        fn place(&self, view: &NonNull<c_void>, frame: Frame) {
            self.ledger
                .borrow_mut()
                .frames
                .insert(address(*view), frame);
        }

        fn set_hidden(&self, view: &NonNull<c_void>, hidden: bool) {
            self.ledger
                .borrow_mut()
                .hidden
                .insert(address(*view), hidden);
        }

        fn detach(&self, view: NonNull<c_void>) -> NonNull<c_void> {
            self.ledger
                .borrow_mut()
                .attached
                .retain(|attached| *attached != address(view));
            view
        }

        /// The overlay's stand-in address (its place in the ledger's
        /// `attached` stack) and its current frame.
        type Overlay = (usize, std::cell::Cell<Frame>);

        fn attach_overlay(
            &self,
            above: &NonNull<c_void>,
            frame: Frame,
        ) -> Option<(usize, std::cell::Cell<Frame>)> {
            if self.refuse_overlay {
                return None;
            }
            let mut ledger = self.ledger.borrow_mut();
            ledger.next_address += 0x10;
            let overlay = 0x9000 + ledger.next_address;
            // Directly above its slot's view, like `addSubview:positioned:
            // Above relativeTo:`.
            let at = ledger
                .attached
                .iter()
                .position(|attached| *attached == address(*above))
                .map_or(ledger.attached.len(), |slot| slot + 1);
            ledger.attached.insert(at, overlay);
            Some((overlay, std::cell::Cell::new(frame)))
        }

        fn place_overlay(&self, overlay: &(usize, std::cell::Cell<Frame>), frame: Frame) {
            overlay.1.set(frame);
        }

        fn overlay_layer(
            &self,
            overlay: &(usize, std::cell::Cell<Frame>),
        ) -> (NonNull<c_void>, (u32, u32)) {
            let frame = overlay.1.get();
            (
                NonNull::new(std::ptr::without_provenance_mut(overlay.0))
                    .expect("a nonzero address is never null"),
                (
                    (frame.width * self.scale) as u32,
                    (frame.height * self.scale) as u32,
                ),
            )
        }

        fn publish_overlay(
            &self,
            overlay_id: u64,
            _layer: NonNull<c_void>,
            size: (u32, u32),
        ) -> bool {
            self.ledger
                .borrow_mut()
                .overlay_events
                .push(format!("publish {overlay_id} {}x{}", size.0, size.1));
            true
        }

        fn withdraw_overlay(&self, overlay_id: u64) {
            self.ledger
                .borrow_mut()
                .overlay_events
                .push(format!("withdraw {overlay_id}"));
        }

        fn request_frame(&self) {
            self.ledger
                .borrow_mut()
                .overlay_events
                .push("frame".to_string());
        }

        fn detach_overlay(&self, overlay: (usize, std::cell::Cell<Frame>)) {
            let mut ledger = self.ledger.borrow_mut();
            ledger.attached.retain(|attached| *attached != overlay.0);
            ledger
                .overlay_events
                .push(format!("detach {:#x}", overlay.0));
        }
    }

    const VIDEO: &str = "dev.frust.test.Video";

    fn create(slot_id: u64) -> ViewCommand {
        ViewCommand::Create {
            slot_id,
            view_type: VIDEO.to_string(),
            params_json: "{\"session\":1}".to_string(),
            interactive: false,
        }
    }

    fn update(slot_id: u64, bounds: Rect, visible: bool) -> ViewCommand {
        ViewCommand::Update {
            slot_id,
            rect: bounds,
            clip: None,
            visible,
            shields: Vec::new(),
            z_index: 0,
        }
    }

    // --- Retain accounting ---------------------------------------------

    #[test]
    fn a_create_then_dispose_exchanges_exactly_one_retain() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(
            &ops,
            &[create(7), update(7, rect(0.0, 0.0, 10.0, 10.0), true)],
        );
        assert_eq!(ledger.borrow().live().len(), 1, "the slot's view is alive");
        assert_eq!(ledger.borrow().attached.len(), 1, "and parented");

        host.apply(&ops, &[ViewCommand::Dispose { slot_id: 7 }]);
        assert_eq!(
            ledger.borrow().live(),
            Vec::<usize>::new(),
            "dispose must hand the factory's one retain back"
        );
        assert_eq!(
            ledger.borrow().over_released(),
            Vec::<usize>::new(),
            "and must not release a second time"
        );
        assert!(ledger.borrow().attached.is_empty(), "and must unparent");
        assert_eq!(factory.create_calls.load(Ordering::SeqCst), 1);
        assert_eq!(factory.dispose_calls.load(Ordering::SeqCst), 1);
        assert_eq!(host.slot_count(), 0);
    }

    #[test]
    fn update_params_lends_the_view_without_changing_the_retain_count() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1)]);
        let view = ledger.borrow().live()[0];

        host.apply(
            &ops,
            &[ViewCommand::UpdateParams {
                slot_id: 1,
                params_json: "{\"session\":2}".to_string(),
            }],
        );

        assert_eq!(
            ledger.borrow().counts[&view],
            1,
            "a lent handle has no Drop and must not change the count"
        );
        assert_eq!(
            ledger.borrow().params[&view],
            vec!["{\"session\":1}".to_string(), "{\"session\":2}".to_string()],
            "the factory must see the new params on the same view"
        );
        assert_eq!(
            factory.create_calls.load(Ordering::SeqCst),
            1,
            "not a recreate"
        );
    }

    #[test]
    fn suspend_returns_every_live_view_to_its_factory() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1), create(2), create(3)]);
        assert_eq!(host.slot_count(), 3);

        host.suspend(&ops);

        assert_eq!(host.slot_count(), 0, "a suspend removes every slot");
        assert_eq!(factory.dispose_calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            ledger.borrow().live(),
            Vec::<usize>::new(),
            "hiding instead of disposing would leak all three"
        );
        assert!(ledger.borrow().attached.is_empty());
    }

    #[test]
    fn a_view_this_host_cannot_adopt_leaves_a_dead_slot() {
        // The refusal path is the null pointer, which carries no retain, so
        // the ledger stays balanced even though nothing was disposed.
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let mut ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        ops.refuse_attach = true;
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(
            &ops,
            &[create(1), update(1, rect(0.0, 0.0, 10.0, 10.0), true)],
        );

        assert_eq!(host.slot_count(), 0);
        assert_eq!(factory.dispose_calls.load(Ordering::SeqCst), 0);
    }

    // --- Replay idempotence --------------------------------------------

    #[test]
    fn a_replayed_create_does_not_build_a_second_view() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(4)]);
        host.apply(&ops, &[create(4)]);

        assert_eq!(
            factory.create_calls.load(Ordering::SeqCst),
            1,
            "an already-applied prefix may arrive twice"
        );
        assert_eq!(host.slot_count(), 1);
    }

    #[test]
    fn an_update_for_an_unknown_slot_is_ignored_not_an_implicit_create() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[update(9, rect(0.0, 0.0, 10.0, 10.0), true)]);
        host.apply(
            &ops,
            &[ViewCommand::UpdateParams {
                slot_id: 9,
                params_json: "{}".to_string(),
            }],
        );

        assert_eq!(factory.create_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.slot_count(), 0);
    }

    #[test]
    fn a_dispose_for_a_slot_already_gone_is_a_no_op() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(2)]);
        host.apply(&ops, &[ViewCommand::Dispose { slot_id: 2 }]);
        host.apply(&ops, &[ViewCommand::Dispose { slot_id: 2 }]);

        assert_eq!(
            factory.dispose_calls.load(Ordering::SeqCst),
            1,
            "a second dispose would over-release the view"
        );
        assert_eq!(ledger.borrow().over_released(), Vec::<usize>::new());
    }

    // --- Placement -----------------------------------------------------

    #[test]
    fn a_created_view_stays_hidden_until_its_first_update_places_it() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1)]);
        let view = ledger.borrow().live()[0];
        assert!(
            ledger.borrow().hidden[&view],
            "showing a view at its default zero frame would flash at the window origin"
        );
        assert!(!ledger.borrow().frames.contains_key(&view));

        host.apply(&ops, &[update(1, rect(5.0, 6.0, 105.0, 56.0), true)]);
        assert!(!ledger.borrow().hidden[&view]);
        assert_eq!(
            ledger.borrow().frames[&view],
            Frame {
                x: 5.0,
                y: 6.0,
                width: 100.0,
                height: 50.0
            }
        );
    }

    #[test]
    fn an_invisible_update_hides_without_disposing() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(
            &ops,
            &[create(1), update(1, rect(0.0, 0.0, 10.0, 10.0), true)],
        );
        host.apply(&ops, &[update(1, rect(0.0, 0.0, 10.0, 10.0), false)]);

        let view = ledger.borrow().live()[0];
        assert!(ledger.borrow().hidden[&view]);
        assert_eq!(host.slot_count(), 1, "hidden is not disposed");
        assert_eq!(factory.dispose_calls.load(Ordering::SeqCst), 0);
    }

    // --- Miss handling -------------------------------------------------

    #[test]
    fn an_unregistered_view_type_is_reported_once_and_leaves_the_slot_dead() {
        let ledger = RetainLedger::new();
        // Nothing registered at all — the shape the demo view types on a
        // sample app's platform page produce.
        let ops = FakeOps::new(&ledger);
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1)]);
        assert!(host.missing.contains(VIDEO), "the miss is recorded");
        assert_eq!(host.slot_count(), 0, "the slot stays dead");

        // A second create for the same view type must not re-report it: the
        // differ re-creates a slot every time its widget is rebuilt, so a
        // per-create warn would be a per-frame log flood.
        host.apply(&ops, &[create(2)]);
        assert_eq!(host.missing.len(), 1);

        // A different view type is its own first report.
        host.apply(
            &ops,
            &[ViewCommand::Create {
                slot_id: 3,
                view_type: "dev.frust.test.Map".to_string(),
                params_json: String::new(),
                interactive: false,
            }],
        );
        assert_eq!(host.missing.len(), 2);
    }

    #[test]
    fn a_miss_survives_a_suspend_so_a_surface_recreate_does_not_re_report_it() {
        let ledger = RetainLedger::new();
        let ops = FakeOps::new(&ledger);
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1)]);
        host.suspend(&ops);
        assert_eq!(host.missing.len(), 1);

        host.apply(&ops, &[create(1)]);
        assert_eq!(
            host.missing.len(),
            1,
            "the replay after a surface recreate must not re-warn"
        );
    }

    #[test]
    fn a_factory_that_declines_leaves_a_dead_slot_without_a_miss_entry() {
        // A decline is a per-slot answer ("this session is gone"), not a
        // missing registration — re-reporting it per create is correct, and
        // the warn-once set must not swallow it.
        let ledger = RetainLedger::new();
        let factory = FakeFactory::declining(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(&ops, &[create(1)]);
        host.apply(&ops, &[create(2)]);

        assert_eq!(factory.create_calls.load(Ordering::SeqCst), 2);
        assert!(host.missing.is_empty());
        assert_eq!(host.slot_count(), 0);
    }

    #[test]
    fn a_panicking_factory_leaves_a_dead_slot_instead_of_unwinding() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::panicking(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory.clone());
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        // Would unwind into the caller — AppKit, in the real host — without
        // the catch.
        host.apply(
            &ops,
            &[create(1), update(1, rect(0.0, 0.0, 10.0, 10.0), true)],
        );

        assert_eq!(host.slot_count(), 0);
        assert_eq!(ledger.borrow().live(), Vec::<usize>::new());
    }

    // --- Overlay surfaces ----------------------------------------------

    fn overlay(overlay_id: u64, above_slot_id: u64, viewport: Rect) -> ViewCommand {
        ViewCommand::Overlay {
            overlay_id,
            above_slot_id,
            rect: viewport,
            command_range: 3..9,
        }
    }

    fn events(ledger: &Rc<RefCell<RetainLedger>>) -> Vec<String> {
        std::mem::take(&mut ledger.borrow_mut().overlay_events)
    }

    #[test]
    fn the_switch_off_ignores_the_overlay_commands() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(false);

        host.apply(
            &ops,
            &[
                create(1),
                update(1, rect(0.0, 0.0, 10.0, 10.0), true),
                overlay(1, 1, rect(0.0, 0.0, 800.0, 600.0)),
                ViewCommand::DisposeOverlay { overlay_id: 1 },
            ],
        );

        assert_eq!(ledger.borrow().attached.len(), 1, "the hosted view alone");
        assert!(host.overlay_layers(&ops).is_empty());
        assert!(events(&ledger).is_empty());
    }

    #[test]
    fn an_overlay_stacks_directly_above_its_slot_and_publishes_its_layer() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(true);

        host.apply(
            &ops,
            &[
                create(1),
                update(1, rect(0.0, 0.0, 10.0, 10.0), true),
                create(2),
                update(2, rect(0.0, 20.0, 10.0, 30.0), true),
                // Content painted between the two views, and content painted
                // over the second one.
                overlay(1, 1, rect(0.0, 0.0, 800.0, 600.0)),
                overlay(2, 2, rect(0.0, 0.0, 800.0, 600.0)),
            ],
        );
        let layers = host.overlay_layers(&ops);
        let [(1, first, _), (2, second, (1600, 1200))] = layers.as_slice() else {
            panic!("two overlays at the window's drawable size: {layers:?}");
        };
        let views = ledger.borrow().live();
        // Bottom to top: slot 1, its overlay, slot 2, its overlay — the
        // between-content stays under the later view.
        assert_eq!(
            ledger.borrow().attached,
            vec![views[0], address(*first), views[1], address(*second)]
        );
        assert_eq!(
            events(&ledger),
            [
                "publish 1 1600x1200",
                "frame",
                "publish 2 1600x1200",
                "frame"
            ]
        );

        // A view hosted later goes to the top, above every overlay.
        host.apply(&ops, &[create(3)]);
        assert_eq!(ledger.borrow().attached.len(), 5);
        assert_eq!(
            ledger.borrow().attached.last().copied(),
            ledger.borrow().live().last().copied()
        );
    }

    #[test]
    fn a_resized_viewport_re_places_and_re_publishes_the_overlay() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(true);
        host.apply(
            &ops,
            &[create(1), overlay(1, 1, rect(0.0, 0.0, 800.0, 600.0))],
        );
        events(&ledger);

        // The same rect again (a range change) touches nothing.
        host.apply(&ops, &[overlay(1, 1, rect(0.0, 0.0, 800.0, 600.0))]);
        assert!(events(&ledger).is_empty());

        host.apply(&ops, &[overlay(1, 1, rect(0.0, 0.0, 1000.0, 700.0))]);
        assert_eq!(events(&ledger), ["publish 1 2000x1400", "frame"]);
    }

    #[test]
    fn a_disposed_overlay_is_withdrawn_before_its_view_goes() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(true);
        host.apply(&ops, &[create(1), overlay(4, 1, rect(0.0, 0.0, 8.0, 6.0))]);
        let (_, layer, _) = host.overlay_layers(&ops)[0];
        events(&ledger);

        host.apply(
            &ops,
            &[
                ViewCommand::DisposeOverlay { overlay_id: 4 },
                ViewCommand::Dispose { slot_id: 1 },
            ],
        );
        assert_eq!(
            events(&ledger),
            [
                "withdraw 4".to_string(),
                format!("detach {:#x}", address(layer))
            ]
        );
        assert!(ledger.borrow().attached.is_empty());
        assert!(host.overlay_layers(&ops).is_empty());
    }

    #[test]
    fn a_suspend_withdraws_every_overlay_before_removing_the_views() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(true);
        host.apply(&ops, &[create(1), overlay(1, 1, rect(0.0, 0.0, 8.0, 6.0))]);
        events(&ledger);

        host.suspend(&ops);
        let suspended = events(&ledger);
        assert_eq!(suspended.first().map(String::as_str), Some("withdraw 1"));
        assert!(suspended[1].starts_with("detach"));
        assert!(ledger.borrow().attached.is_empty());
        assert!(ledger.borrow().over_released().is_empty());

        // The replay after the surface returns stacks it again.
        host.apply(&ops, &[create(1), overlay(1, 1, rect(0.0, 0.0, 8.0, 6.0))]);
        assert_eq!(host.overlay_layers(&ops).len(), 1);
    }

    #[test]
    fn an_overlay_without_a_slot_view_or_a_view_of_its_own_is_skipped() {
        let ledger = RetainLedger::new();
        let factory = FakeFactory::new(&ledger);
        let mut ops = FakeOps::new(&ledger).with(VIDEO, factory);
        let mut host = ViewHost::<FakeOps>::with_overlays(true);

        // A dead slot: nothing to sit above.
        host.apply(&ops, &[overlay(1, 9, rect(0.0, 0.0, 8.0, 6.0))]);
        assert!(host.overlay_layers(&ops).is_empty());

        ops.refuse_overlay = true;
        host.apply(&ops, &[create(1), overlay(2, 1, rect(0.0, 0.0, 8.0, 6.0))]);
        assert_eq!(host.slot_count(), 1);
        assert_eq!(ledger.borrow().attached.len(), 1);
        assert!(host.overlay_layers(&ops).is_empty());
        assert!(events(&ledger).is_empty());
    }
}
