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
//! # Status: fetch seam and loader core
//!
//! The crate carries the [`ImageFetcher`] trait, the default
//! `ReqwestFetcher` (feature `reqwest`), and [`ImageLoader`]: single-flight
//! fetches, decode/downscale on the blocking pool, and a byte-budgeted
//! memory LRU. The view and disk cache arrive in a later phase.

mod decode;
pub mod fetch;
mod loader;
mod memory_cache;
#[cfg(feature = "reqwest")]
mod reqwest_fetcher;

pub use decode::{DecodeError, TargetPx, decode_source};
pub use fetch::*;
pub use loader::{ImageLoader, LoadError, LoadFuture, LoadRequest, LoaderConfig};
pub use memory_cache::MemoryStats;
#[cfg(feature = "reqwest")]
pub use reqwest_fetcher::ReqwestFetcher;
