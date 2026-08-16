//! The Win32 boundary: this crate's sanctioned-unsafe zone, and the only
//! module that names a `windows-sys`/`winit`-Windows-extension API.
//!
//! Every `unsafe` block in `frust-shell-windows` lives here, mirroring
//! `frust-shell-android`'s `jni_glue` and `frust-shell-ios`'s `ffi_glue` (see
//! `docs/SHELLS_ARCHITECTURE.md`'s sanctioned-unsafe-zones convention). There
//! are exactly three of them, each with its own `SAFETY` note:
//!
//! 1. [`set_app_user_model_id`] — `SetCurrentProcessExplicitAppUserModelID`
//!    with a NUL-terminated UTF-16 buffer that outlives the call.
//! 2. `translate_accelerator` — the `TranslateAcceleratorW` inside the
//!    `EventLoopBuilderExtWindows::with_msg_hook` closure body.
//! 3. `attach_menu_to_hwnd` — muda's `Menu::init_for_hwnd`, whose contract is
//!    "`hwnd` must be a valid window HWND".
//!
//! (The last two are named without a doc link on purpose: they exist only in
//! the Windows half of this module, so a link to either would dangle in the
//! docs an off-target build renders.)
//!
//! # Inert off Windows
//!
//! Each entry point is defined twice: the real Win32 body under
//! `cfg(target_os = "windows")`, and a do-nothing stand-in otherwise, so the
//! hooks in [`crate`] read the same on every host and this crate compiles (as
//! an inert no-op) on the Linux dev machine. The two bodies sit next to each
//! other on purpose — an off-target build's whole behavioral difference is
//! visible in one file.
//!
//! # Failure is degradation, never a broken message loop
//!
//! Nothing here returns an error a caller must handle: an identity call that
//! fails, a window whose HWND cannot be recovered, a menu that will not attach
//! — each logs and leaves the app running without that one integration. The
//! accelerator hook is the sharpest case: until [`AcceleratorTable::set`] is
//! called it holds no table and the hook returns `false` for every message, so
//! the failure mode is menus without keyboard shortcuts rather than a message
//! loop that swallows input.

use std::cell::Cell;
use std::rc::Rc;

use frust_shell_desktop::config::IconData;
use frust_shell_desktop::extensions::DesktopEventLoopBuilder;
use winit::window::{Window, WindowAttributes};

/// The `HACCEL` the message hook translates against, shared between the
/// builder-stage hook closure and the menu that owns the table.
///
/// The handle is not known when the hook is installed (the menu is built two
/// hooks later, once there is a window to attach it to), so the closure
/// captures this slot instead and re-reads it per message. `0` means "no
/// accelerator table" — the state before a menu is installed, and again after
/// one is dropped ([`clear`](Self::clear)), since muda's `HACCEL` is valid only
/// as long as its `Menu` is.
///
/// `Rc<Cell<_>>` rather than an atomic: the hook closure and every
/// `DesktopExtensions` hook run on the one event-loop thread, and
/// `EventLoopBuilderExtWindows::with_msg_hook` requires only `'static`, not
/// `Send`.
#[derive(Debug, Clone, Default)]
pub(crate) struct AcceleratorTable(Rc<Cell<isize>>);

impl AcceleratorTable {
    /// An empty table — the hook translates nothing until [`set`](Self::set).
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Publish the `HACCEL` of a live menu.
    pub(crate) fn set(&self, haccel: isize) {
        self.0.set(haccel);
    }

    /// Forget the table — called when the owning menu is dropped, so the hook
    /// can never translate against a dangling `HACCEL`.
    pub(crate) fn clear(&self) {
        self.0.set(0);
    }

    /// The current `HACCEL`, or `0` for none.
    fn get(&self) -> isize {
        self.0.get()
    }
}

/// Give the process an explicit AppUserModelID, so the taskbar groups this
/// app's windows under its own identity (and notifications attribute to it)
/// rather than to the host `.exe`'s inferred one.
///
/// Best-effort: a failing `HRESULT` is logged and ignored — the shell has
/// nothing to fall back to and the app is perfectly usable without it.
///
/// Must run before the first window is created, which is why the caller drives
/// it from the builder-stage hook (the first hook to fire).
#[cfg(target_os = "windows")]
pub(crate) fn set_app_user_model_id(app_id: &str) {
    // NUL-terminated UTF-16, held in a local for the whole call: `PCWSTR` is a
    // borrowed pointer, and Windows copies the string before returning.
    let wide: Vec<u16> = app_id.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that lives across the
    // call, which is the whole of `SetCurrentProcessExplicitAppUserModelID`'s
    // contract; it takes no ownership of the pointer.
    let hr = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(wide.as_ptr())
    };
    if hr < 0 {
        log::warn!(
            "frust-shell-windows: SetCurrentProcessExplicitAppUserModelID({app_id}) failed (HRESULT {hr:#010x})"
        );
    } else {
        log::debug!("frust-shell-windows: AppUserModelID set to {app_id}");
    }
}

// Inert off Windows: there is no shell identity to set.
#[cfg(not(target_os = "windows"))]
pub(crate) fn set_app_user_model_id(app_id: &str) {
    let _ = app_id;
}

/// Install the message hook that turns a menu accelerator into a menu command
/// before winit consumes the keystroke.
///
/// winit accepts this only on the event-loop *builder*, before `build()` — so
/// it is installed before any menu exists and reads `table` per message
/// instead of capturing a handle.
#[cfg(target_os = "windows")]
pub(crate) fn install_accelerator_hook(
    builder: &mut DesktopEventLoopBuilder,
    table: AcceleratorTable,
) {
    use winit::platform::windows::EventLoopBuilderExtWindows;

    builder.with_msg_hook(move |msg| translate_accelerator(&table, msg));
}

// Inert off Windows: no message loop to hook.
#[cfg(not(target_os = "windows"))]
pub(crate) fn install_accelerator_hook(
    builder: &mut DesktopEventLoopBuilder,
    table: AcceleratorTable,
) {
    let _ = (builder, table);
}

/// The message-hook body: `true` means "handled — winit must not translate or
/// dispatch this message".
///
/// Returns `false` for every message while the table is empty, so an app whose
/// menu failed to install keeps a completely ordinary message loop.
///
/// A `true` here is also why the menu queue owns its own wake: the keystroke
/// never becomes a winit event, so nothing on winit's side marks the loop
/// dirty. `TranslateAcceleratorW` turns it into a `WM_COMMAND` the window
/// procedure feeds to muda, whose handler queues the activation in
/// [`crate::menu`]'s bridge and requests the redraw that delivers it — the same
/// path a mouse click on the menu takes.
#[cfg(target_os = "windows")]
fn translate_accelerator(table: &AcceleratorTable, msg: *const std::ffi::c_void) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HACCEL, MSG, TranslateAcceleratorW};

    let haccel = table.get();
    if haccel == 0 || msg.is_null() {
        return false;
    }
    let msg = msg.cast::<MSG>();
    // SAFETY: winit's `with_msg_hook` contract is that it passes a pointer to
    // the `MSG` it just took from `PeekMessageW`, valid for this call; `MSG` is
    // a plain C struct with one layout, so reading it through this crate's own
    // `windows-sys` binding of it is the same read winit's own doc example
    // makes. `haccel` is non-zero and belongs to a `muda::Menu` the extension
    // still holds (`AcceleratorTable::clear` runs when that menu is dropped).
    // `TranslateAcceleratorW` only reads through both pointers.
    unsafe { TranslateAcceleratorW((*msg).hwnd, haccel as HACCEL, msg) == 1 }
}

/// Attach a native menu bar to the window's `HWND`.
///
/// Returns whether it took: a refusal is logged and leaves the window
/// menu-less rather than failing window creation.
#[cfg(target_os = "windows")]
pub(crate) fn attach_menu_to_hwnd(menu: &muda::Menu, hwnd: isize) -> bool {
    // SAFETY: `init_for_hwnd`'s contract is that `hwnd` is a valid window
    // handle. It came from `window_hwnd` below — i.e. straight out of the live
    // `winit::Window`'s own `raw-window-handle` — and the window is alive for
    // the whole of this call (the caller holds it).
    match unsafe { menu.init_for_hwnd(hwnd) } {
        Ok(()) => true,
        Err(err) => {
            log::warn!("frust-shell-windows: could not attach the native menu bar: {err}");
            false
        }
    }
}

/// The window's `HWND`, as the `isize` muda's Win32 entry points take.
///
/// `None` (logged) if winit reports anything but a Win32 handle — which cannot
/// happen for a window this crate's own shell created, but is a refusal rather
/// than a panic all the same.
#[cfg(target_os = "windows")]
pub(crate) fn window_hwnd(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match window.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
        Ok(other) => {
            log::warn!("frust-shell-windows: window handle is not Win32 ({other:?})");
            None
        }
        Err(err) => {
            log::warn!("frust-shell-windows: no window handle available: {err}");
            None
        }
    }
}

// Inert off Windows: nothing has an HWND.
#[cfg(not(target_os = "windows"))]
pub(crate) fn window_hwnd(window: &Window) -> Option<isize> {
    let _ = window;
    None
}

/// Attach the app's icon to the window attributes, for both places Windows
/// shows one.
///
/// winit's cross-platform `with_window_icon` reaches `ICON_SMALL` only (the
/// titlebar corner and the alt-tab row's small form); the taskbar button and
/// the large alt-tab form read `ICON_BIG`, which is winit's Windows-only
/// `with_taskbar_icon`. Setting one and not the other is the classic
/// half-branded window, so both are set from the one [`IconData`].
///
/// A malformed icon is logged and dropped — `IconData` already pins
/// `rgba.len() == width * height * 4`, so `Icon::from_rgba` can only fail on a
/// size Windows itself refuses.
#[cfg(target_os = "windows")]
pub(crate) fn with_icons(attributes: WindowAttributes, icon: &IconData) -> WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    use winit::window::Icon;

    let icon = match Icon::from_rgba(icon.rgba().to_vec(), icon.width(), icon.height()) {
        Ok(icon) => icon,
        Err(err) => {
            log::warn!("frust-shell-windows: ignoring the configured window icon: {err}");
            return attributes;
        }
    };
    attributes
        .with_window_icon(Some(icon.clone()))
        .with_taskbar_icon(Some(icon))
}

// Inert off Windows: window/taskbar icons are another shell's business.
#[cfg(not(target_os = "windows"))]
pub(crate) fn with_icons(attributes: WindowAttributes, icon: &IconData) -> WindowAttributes {
    let _ = icon;
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_accelerator_table_is_empty() {
        assert_eq!(AcceleratorTable::new().get(), 0);
    }

    #[test]
    fn a_published_table_is_visible_through_every_clone() {
        // The hook closure holds a clone; the menu that owns the `HACCEL`
        // publishes through another. Both must see the one slot.
        let table = AcceleratorTable::new();
        let hook_side = table.clone();
        table.set(0x1234);
        assert_eq!(hook_side.get(), 0x1234);
        table.clear();
        assert_eq!(hook_side.get(), 0);
    }
}
