//! Placement of the standard window buttons (close, minimize, zoom) under
//! [`Decorations::NoTitleBar`], from the app's [`TitleButtons`].
//!
//! winit 0.30 exposes no placement API for these buttons, so this is AppKit
//! work — the technique Electron's `RedrawTrafficLights` uses: move the three
//! `NSButton`s inside their titlebar view, growing the titlebar container when
//! the requested band is taller than it, and re-run that after every event
//! that makes AppKit lay the titlebar out again (a resize, leaving fullscreen).
//!
//! The module is half gated, like `platform_view`: the geometry
//! ([`placements`]) and the "is placement wanted" decision ([`wanted`]) are
//! plain Rust compiled and unit-tested on every host; only the AppKit half
//! that reads and moves the buttons is `cfg(target_os = "macos")`.

use frust_shell_desktop::{Decorations, TitleButtons};

#[cfg(target_os = "macos")]
pub(crate) use appkit::{WindowButtonObserver, apply};

/// One button's frame in its superview's coordinates, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ButtonFrame {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

impl ButtonFrame {
    /// Whether AppKit has given this button a size yet; a zero frame carries
    /// no native spacing to preserve.
    pub(crate) fn is_laid_out(&self) -> bool {
        self.w > 0.0 && self.h > 0.0
    }
}

/// The placement the config asks for: `Some` only for
/// [`Decorations::NoTitleBar`] with [`TitleButtons`] set. `Full` and `None`
/// (and an unset placement) leave the buttons to AppKit and touch nothing.
///
/// Geometry that fails [`valid_geometry`] is refused here (warned once per
/// call), so AppKit's own layout stays.
pub(crate) fn wanted(
    decorations: Decorations,
    buttons: Option<TitleButtons>,
) -> Option<TitleButtons> {
    match decorations {
        Decorations::NoTitleBar => buttons.filter(|b| {
            let ok = valid_geometry(*b);
            if !ok {
                log::warn!(
                    "frust-shell-macos: title buttons {b:?} are not finite with leading >= 0 and \
                     band_height > 0; window buttons left at the OS position"
                );
            }
            ok
        }),
        Decorations::Full | Decorations::None => None,
    }
}

/// Whether `leading` and `band_height` can be placed: finite, `leading`
/// non-negative and `band_height` positive.
pub(crate) fn valid_geometry(buttons: TitleButtons) -> bool {
    buttons.leading.is_finite()
        && buttons.band_height.is_finite()
        && buttons.leading >= 0.0
        && buttons.band_height > 0.0
}

/// Whether a placement may mutate anything: never in fullscreen (AppKit owns
/// that layout), and only with laid-out buttons and valid geometry.
pub(crate) fn should_place(is_fullscreen: bool, laid_out: bool, geometry_ok: bool) -> bool {
    !is_fullscreen && laid_out && geometry_ok
}

/// New bottom-left origins for the close, minimize and zoom buttons (in that
/// order) inside a superview `container_height` tall, in AppKit's unflipped
/// coordinates (y grows upward).
///
/// Every button shifts horizontally by the same amount, so the close button's
/// left edge lands on `leading` and AppKit's native inter-button spacing is
/// kept; each is centred vertically in the top `band_height` of the container.
/// A band taller than the container is not clamped here — the AppKit caller
/// grows the container first.
pub(crate) fn placements(
    leading: f64,
    band_height: f64,
    container_height: f64,
    frames: [ButtonFrame; 3],
) -> [(f64, f64); 3] {
    let shift = leading - frames[0].x;
    frames.map(|frame| {
        (
            frame.x + shift,
            container_height - band_height / 2.0 - frame.h / 2.0,
        )
    })
}

/// The AppKit half: this crate's second AppKit zone beside `appkit_glue`
/// (sanctioned FFI, `docs/CODE_STANDARDS.md`), with the same per-site
/// `SAFETY:` discipline and the same no-unwind rule for the AppKit callback.
///
/// # The `unsafe` in this module
///
/// 1. `define_class!`'s `#[unsafe(super(NSObject))]` / `#[unsafe(method(…))]`
///    — the observer class and the selector the notification center calls.
/// 2. `msg_send![super(this), init]` — `NSObject`'s designated initializer.
/// 3. `Retained::retain` on winit's raw-window-handle `NSView` — how the
///    `NSWindow` is reached (the `platform_view` `content_view` shape).
/// 4. `NSView::superview` (twice) — an unretained-property getter.
/// 5. `NSNotificationCenter::addObserver_selector_name_object` (once per
///    name) plus the reads of the `NSWindow*Notification` `extern` statics.
/// 6. `NSNotificationCenter::removeObserver` in `Drop` — the center holds the
///    observer unretained, so the registration must end before it drops.
#[cfg(target_os = "macos")]
mod appkit {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};

    use frust_shell_desktop::TitleButtons;
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{
        NSView, NSWindow, NSWindowButton, NSWindowDidBecomeKeyNotification,
        NSWindowDidExitFullScreenNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
        NSWindowWillEnterFullScreenNotification,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    use super::{ButtonFrame, placements, should_place, valid_geometry};

    /// Set once a placement has been skipped, so a hierarchy AppKit lays out
    /// differently from the one assumed here logs once, not on every resize.
    static SKIP_REPORTED: AtomicBool = AtomicBool::new(false);

    fn report_skip(reason: &str) {
        if !SKIP_REPORTED.swap(true, Ordering::Relaxed) {
            log::warn!("frust-shell-macos: window buttons left at the OS position: {reason}");
        }
    }

    /// Place `window`'s standard buttons as `buttons` asks. Must run on the
    /// main thread; anything missing (a button, a superview) is logged once
    /// and leaves the buttons where AppKit put them.
    /// Returns whether the buttons were placed.
    pub(crate) fn apply(window: &Window, buttons: TitleButtons) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            report_skip("not on the main thread");
            return false;
        };
        match ns_window(window, mtm) {
            Some(ns_window) => place(&ns_window, buttons),
            None => {
                report_skip("no NSWindow behind the winit window");
                false
            }
        }
    }

    /// The `NSWindow` behind `window`, through winit's raw-window-handle
    /// content view.
    fn ns_window(window: &Window, _mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: `ns_view` is the live content view winit owns for `window`,
        // which the caller borrows for this whole call; `Retained::retain`
        // takes a +1 of our own instead of stealing winit's.
        let view: Retained<NSView> =
            unsafe { Retained::retain(appkit.ns_view.cast::<NSView>().as_ptr()) }?;
        view.window()
    }

    /// Place the buttons; `true` only when they were moved. Every
    /// precondition is read before any view changes, so a skip mutates nothing.
    fn place(window: &NSWindow, buttons: TitleButtons) -> bool {
        if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
            log::debug!("frust-shell-macos: fullscreen, window buttons left to AppKit");
            return false;
        }
        let kinds = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ];
        let [Some(close), Some(minimize), Some(zoom)] =
            kinds.map(|kind| window.standardWindowButton(kind))
        else {
            report_skip("a standard window button is missing");
            return false;
        };
        // SAFETY (both getters): `superview` is unretained in AppKit; the
        // button is in the live window's frame-view hierarchy, which the
        // window retains, so each superview is alive while it is read and the
        // returned `Retained` holds its own +1 from here on.
        let Some(titlebar) = (unsafe { close.superview() }) else {
            report_skip("the close button has no superview");
            return false;
        };
        let Some(container) = (unsafe { titlebar.superview() }) else {
            report_skip("the titlebar view has no container");
            return false;
        };

        let views = [close, minimize, zoom];
        let frames = views.each_ref().map(|view| {
            let frame = view.frame();
            ButtonFrame {
                x: frame.origin.x,
                y: frame.origin.y,
                w: frame.size.width,
                h: frame.size.height,
            }
        });
        if !should_place(
            false,
            frames.iter().all(ButtonFrame::is_laid_out),
            valid_geometry(buttons),
        ) {
            report_skip("the buttons have no size yet or the geometry is invalid");
            return false;
        }

        // Past every check: grow the titlebar container to the band, anchored
        // at the window's top (unflipped: its origin is the band's bottom edge).
        let container_frame = container.frame();
        if container_frame.size.height < buttons.band_height {
            let window_height = window.frame().size.height;
            container.setFrame(NSRect::new(
                NSPoint::new(
                    container_frame.origin.x,
                    window_height - buttons.band_height,
                ),
                NSSize::new(container_frame.size.width, buttons.band_height),
            ));
        }
        // The buttons' own coordinate space: the titlebar view, re-read after
        // the container grew (it autoresizes with the container).
        let titlebar_height = titlebar.frame().size.height;
        let origins = placements(
            buttons.leading,
            buttons.band_height,
            titlebar_height,
            frames,
        );
        log::debug!(
            "frust-shell-macos: window buttons {frames:?} in a {titlebar_height} pt titlebar -> \
             {origins:?}"
        );
        for (view, (x, y)) in views.iter().zip(origins) {
            view.setFrameOrigin(NSPoint::new(x, y));
        }
        true
    }

    /// The observer's state: the requested placement plus the one-shot and
    /// fullscreen-transition flags (main-thread only, hence `Cell`).
    struct ObserverState {
        buttons: TitleButtons,
        /// A placement has succeeded; the post-show retry is then unneeded.
        placed: Cell<bool>,
        /// The first `DidBecomeKey` has been handled; later ones are ignored.
        key_seen: Cell<bool>,
        /// Between `WillEnterFullScreen` and `DidExitFullScreen`.
        in_fullscreen: Cell<bool>,
    }

    define_class!(
        // SAFETY:
        // - `NSObject` imposes no subclassing requirements.
        // - `FrustWindowButtonObserver` does not implement `Drop` (the handle
        //   that owns it does).
        #[unsafe(super(NSObject))]
        // Window notifications are posted on the main thread, where the
        // buttons may be touched.
        #[thread_kind = MainThreadOnly]
        #[ivars = ObserverState]
        struct FrustWindowButtonObserver;

        impl FrustWindowButtonObserver {
            /// `NSWindowDidResizeNotification`: AppKit has laid the titlebar
            /// out again, so re-place the buttons unless a fullscreen
            /// transition is under way.
            #[unsafe(method(frustWindowButtonsNeedPlacement:))]
            fn needs_placement(&self, notification: &NSNotification) {
                if self.ivars().in_fullscreen.get() {
                    return;
                }
                self.replace_on(notification);
            }

            /// `NSWindowWillEnterFullScreenNotification`: ignore resizes
            /// until the exit.
            #[unsafe(method(frustWindowWillEnterFullScreen:))]
            fn will_enter_fullscreen(&self, _notification: &NSNotification) {
                self.ivars().in_fullscreen.set(true);
            }

            /// `NSWindowDidExitFullScreenNotification`: re-place.
            #[unsafe(method(frustWindowDidExitFullScreen:))]
            fn did_exit_fullscreen(&self, notification: &NSNotification) {
                self.ivars().in_fullscreen.set(false);
                self.replace_on(notification);
            }

            /// `NSWindowDidBecomeKeyNotification`: the window is shown, so
            /// retry once if the first placement was skipped.
            #[unsafe(method(frustWindowDidBecomeKey:))]
            fn did_become_key(&self, notification: &NSNotification) {
                let state = self.ivars();
                if state.key_seen.replace(true) || state.placed.get() {
                    return;
                }
                self.replace_on(notification);
            }
        }

        unsafe impl NSObjectProtocol for FrustWindowButtonObserver {}
    );

    impl FrustWindowButtonObserver {
        /// Place on the notification's window, recording success.
        fn replace_on(&self, notification: &NSNotification) {
            let buttons = self.ivars().buttons;
            // A panic would unwind into AppKit's dispatch; swallow and log.
            let result = catch_unwind(AssertUnwindSafe(|| {
                let object = notification.object();
                match object.as_deref().and_then(|o| o.downcast_ref::<NSWindow>()) {
                    Some(window) => {
                        if place(window, buttons) {
                            self.ivars().placed.set(true);
                        }
                    }
                    None => report_skip("a window notification carried no NSWindow"),
                }
            }));
            if result.is_err() {
                log::error!("frust-shell-macos: panic while re-placing the window buttons");
            }
        }

        fn new(mtm: MainThreadMarker, buttons: TitleButtons, placed: bool) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ObserverState {
                buttons,
                placed: Cell::new(placed),
                key_seen: Cell::new(false),
                in_fullscreen: Cell::new(false),
            });
            // SAFETY: `init` is `NSObject`'s designated initializer, sent once
            // to a freshly allocated instance whose ivars are already set.
            unsafe { msg_send![super(this), init] }
        }
    }

    /// A live registration re-placing one window's buttons after a resize, a
    /// fullscreen exit or (once) the first become-key; dropping it unregisters (see the module docs).
    pub(crate) struct WindowButtonObserver {
        observer: Retained<FrustWindowButtonObserver>,
    }

    impl WindowButtonObserver {
        /// Observe `window`'s own resize, fullscreen and become-key
        /// notifications, or `None` (logged once) off the main thread or
        /// without an `NSWindow`. `placed` is whether the first placement
        /// already succeeded; if not, the first become-key retries it.
        pub(crate) fn install(
            window: &Window,
            buttons: TitleButtons,
            placed: bool,
        ) -> Option<Self> {
            let Some(mtm) = MainThreadMarker::new() else {
                report_skip("not on the main thread");
                return None;
            };
            let Some(ns_window) = ns_window(window, mtm) else {
                report_skip("no NSWindow behind the winit window");
                return None;
            };
            let observer = FrustWindowButtonObserver::new(mtm, buttons, placed);
            let center = NSNotificationCenter::defaultCenter();
            // SAFETY (the statics): AppKit's own notification-name statics,
            // read after AppKit is loaded (this runs from a live event loop).
            let registrations = unsafe {
                [
                    (
                        sel!(frustWindowButtonsNeedPlacement:),
                        NSWindowDidResizeNotification,
                    ),
                    (
                        sel!(frustWindowWillEnterFullScreen:),
                        NSWindowWillEnterFullScreenNotification,
                    ),
                    (
                        sel!(frustWindowDidExitFullScreen:),
                        NSWindowDidExitFullScreenNotification,
                    ),
                    (
                        sel!(frustWindowDidBecomeKey:),
                        NSWindowDidBecomeKeyNotification,
                    ),
                ]
            };
            for (selector, name) in registrations {
                // SAFETY:
                // - `observer` implements each selector with the single
                //   `NSNotification` signature the center invokes, and stays
                //   alive (retained by the returned handle) until `Drop`
                //   unregisters it — the center holds it unretained.
                // - The object filter is the window itself, so only its own
                //   notifications arrive; it is compared, not retained.
                unsafe {
                    center.addObserver_selector_name_object(
                        &observer,
                        selector,
                        Some(name),
                        Some(&ns_window),
                    );
                }
            }
            Some(Self { observer })
        }
    }

    impl Drop for WindowButtonObserver {
        fn drop(&mut self) {
            let center = NSNotificationCenter::defaultCenter();
            // SAFETY: `observer` is the object registered in `install` and is
            // still alive here (its `Retained` drops after this body); this
            // removes every one of its registrations.
            unsafe { center.removeObserver(&self.observer) };
        }
    }

    impl std::fmt::Debug for WindowButtonObserver {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("WindowButtonObserver").finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AppKit's own layout for a standard titled window: 14 pt buttons at
    /// x = 7/27/47, centred in a 28 pt titlebar.
    fn native_frames() -> [ButtonFrame; 3] {
        [7.0, 27.0, 47.0].map(|x| ButtonFrame {
            x,
            y: 7.0,
            w: 14.0,
            h: 14.0,
        })
    }

    #[test]
    fn the_native_layout_is_a_fixed_point() {
        let origins = placements(7.0, 28.0, 28.0, native_frames());
        assert_eq!(origins, [(7.0, 7.0), (27.0, 7.0), (47.0, 7.0)]);
    }

    #[test]
    fn leading_shifts_all_three_and_keeps_the_native_spacing() {
        let origins = placements(16.0, 40.0, 40.0, native_frames());
        assert_eq!(origins, [(16.0, 13.0), (36.0, 13.0), (56.0, 13.0)]);
    }

    #[test]
    fn a_band_shorter_than_the_container_centres_in_the_top_of_it() {
        // Unflipped: the top 20 pt of a 40 pt container is y in 20..40.
        let origins = placements(7.0, 20.0, 40.0, native_frames());
        assert_eq!(origins[0], (7.0, 23.0));
    }

    #[test]
    fn a_band_taller_than_the_container_is_not_clamped() {
        // The AppKit caller grows the container first; this is the raw math.
        let origins = placements(7.0, 40.0, 28.0, native_frames());
        assert_eq!(origins[0], (7.0, 1.0));
    }

    #[test]
    fn an_empty_frame_is_not_laid_out() {
        let empty = ButtonFrame {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        assert!(!empty.is_laid_out());
        assert!(native_frames().iter().all(ButtonFrame::is_laid_out));
    }

    #[test]
    fn geometry_must_be_finite_non_negative_with_a_positive_band() {
        let ok = |l, b| valid_geometry(TitleButtons::new(l, b));
        assert!(ok(0.0, 1.0));
        assert!(ok(16.0, 40.0));
        assert!(!ok(f64::NAN, 40.0));
        assert!(!ok(16.0, f64::NAN));
        assert!(!ok(f64::INFINITY, 40.0));
        assert!(!ok(16.0, f64::INFINITY));
        assert!(!ok(f64::NEG_INFINITY, 40.0));
        assert!(!ok(-1.0, 40.0));
        assert!(!ok(16.0, 0.0));
        assert!(!ok(16.0, -4.0));
    }

    #[test]
    fn invalid_geometry_is_not_wanted() {
        let bad = Some(TitleButtons::new(f64::NAN, 40.0));
        assert_eq!(wanted(Decorations::NoTitleBar, bad), None);
    }

    #[test]
    fn placement_needs_windowed_laid_out_valid() {
        assert!(should_place(false, true, true));
        assert!(!should_place(true, true, true));
        assert!(!should_place(false, false, true));
        assert!(!should_place(false, true, false));
    }

    #[test]
    fn placement_is_wanted_only_without_a_title_bar() {
        let buttons = Some(TitleButtons::new(16.0, 40.0));
        assert_eq!(wanted(Decorations::NoTitleBar, buttons), buttons);
        assert_eq!(wanted(Decorations::NoTitleBar, None), None);
        assert_eq!(wanted(Decorations::Full, buttons), None);
        assert_eq!(wanted(Decorations::None, buttons), None);
    }
}
