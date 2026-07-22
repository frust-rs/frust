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
