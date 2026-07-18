//! Desktop preview shell: the primary dev-loop platform embedding (spec §12.9).
//!
//! Wraps winit 0.30's [`ApplicationHandler`](winit::application::ApplicationHandler)
//! event loop around the ForgeKit render stack (`forgekit-render` + vello), so a
//! `cargo run` opens the app in a native window with a sub-second rebuild loop
//! and no mobile toolchain. The facade crate (`forgekit`) drives this via
//! [`run_desktop`].

mod app_handler;
mod logger;

pub use app_handler::run_desktop;
