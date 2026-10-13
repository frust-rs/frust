//! `frust-network-image`: a network image view for frust apps — URL images
//! with a memory cache and a disk cache.
//!
//! # Charter: a facade-tier plugin, no OS arm
//!
//! This crate depends on the `frust` facade (plus the `frust-paths` leaf for
//! the disk-cache directory) and never on `frust-plugin` or any platform FFI
//! crate: fetching is pure Rust (reqwest over rustls with the ring provider
//! and the Mozilla `webpki-roots` trust store on every target), so there is
//! no Android/iOS/desktop backend to split by platform. An app adds this
//! crate to its own `Cargo.toml` alongside `frust`; the facade does not
//! depend on or re-export it.
//!
//! # Contents
//!
//! [`network_image`] is the app-facing view: placeholder, error slot, fit,
//! decode target, fade-in and tap-to-retry. It loads through the
//! [`ImageLoader`] that `NetworkImages::install()` (feature `reqwest`) or
//! [`NetworkImages::install_with`] sets up from `frust::app!`'s setup block,
//! or a per-view `.loader(..)` override. The loader runs single-flight
//! fetches through an [`ImageFetcher`] (default `ReqwestFetcher`), decodes and
//! downscales on the blocking pool, and caches in a byte-budgeted memory LRU
//! backed by a persistent [`DiskCache`] of raw response bytes (opened lazily
//! on the first memory miss).

mod decode;
mod disk_cache;
pub mod fetch;
mod install;
mod loader;
mod memory_cache;
#[cfg(feature = "reqwest")]
mod reqwest_fetcher;
mod view;

pub use decode::{DecodeError, TargetPx, decode_source};
pub use disk_cache::{DiskCache, DiskStats};
pub use fetch::*;
pub use install::NetworkImages;
pub use loader::{DiskCacheConfig, ImageLoader, LoadError, LoadFuture, LoadRequest, LoaderConfig};
pub use memory_cache::MemoryStats;
#[cfg(feature = "reqwest")]
pub use reqwest_fetcher::ReqwestFetcher;
pub use view::{LoadState, NetworkImage, NetworkImageView, NetworkImageWidget, network_image};
