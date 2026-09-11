//! Desktop preview shell **and shared desktop core**: the primary dev-loop
//! platform embedding, and the cross-platform half every per-OS desktop shell
//! crate builds on.
//!
//! Wraps winit 0.30's [`ApplicationHandler`](winit::application::ApplicationHandler)
//! event loop around the Frust render stack (`frust-render` + `frust-engine`), so a
//! `cargo run` opens the app in a native window with a sub-second rebuild loop
//! and no mobile toolchain. The facade crate (`frust`) drives this via
//! [`run_desktop`] (zero-config) or [`run_desktop_with`] (with the app's
//! [`DesktopConfig`] identity and a per-OS [`DesktopExtensions`] set — see the
//! [`extensions`] module for the hook contract).

mod app_handler;
mod cache;
pub mod config;
pub mod extensions;
mod logger;
mod render;

pub use app_handler::{run_desktop, run_desktop_with};
pub use config::{DEFAULT_APP_NAME, DesktopConfig, IconData, MenuItemSpec, MenuRole, MenuSpec};
pub use extensions::{CloseAction, DesktopEventLoopBuilder, DesktopExtensions, NoExtensions};

/// The desktop event loop's user-event type, `pub` only so
/// [`DesktopEventLoopBuilder`] (the builder-stage hook's parameter) can name
/// it. `#[doc(hidden)]` keeps it out of published docs — the same escape
/// [`paced_wake`] uses. [`PasteText`] rides along for the same reason and no
/// other: it is the payload of that type's clipboard-answer variant (the text
/// the clipboard worker thread read), and a variant carrying a type the crate
/// root cannot name is a lint, not an API.
#[doc(hidden)]
pub use app_handler::{PasteText, ShellUserEvent};

/// Pure, winit-free decision logic behind `ShellHandler`'s paced-wake
/// mechanism (see the module's own docs). `run_desktop`/`ShellHandler` are
/// this crate's only real public surface — this module is `pub` purely so
/// `tests/paced_wake_integration.rs` can drive it as a black box with a fake
/// advancing clock; `#[doc(hidden)]` keeps it out of published docs.
#[doc(hidden)]
pub mod paced_wake;
