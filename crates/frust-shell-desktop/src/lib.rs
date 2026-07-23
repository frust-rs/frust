//! Desktop preview shell: the primary dev-loop platform embedding (spec §12.9).
//!
//! Wraps winit 0.30's [`ApplicationHandler`](winit::application::ApplicationHandler)
//! event loop around the Frust render stack (`frust-render` + vello), so a
//! `cargo run` opens the app in a native window with a sub-second rebuild loop
//! and no mobile toolchain. The facade crate (`frust`) drives this via
//! [`run_desktop`].

mod app_handler;
mod cache;
mod logger;
mod render;

pub use app_handler::run_desktop;

/// Pure, winit-free decision logic behind `ShellHandler`'s paced-wake
/// mechanism (see the module's own docs). `run_desktop`/`ShellHandler` are
/// this crate's only real public surface — this module is `pub` purely so
/// `tests/paced_wake_integration.rs` can drive it as a black box with a fake
/// advancing clock; `#[doc(hidden)]` keeps it out of published docs.
#[doc(hidden)]
pub mod paced_wake;
