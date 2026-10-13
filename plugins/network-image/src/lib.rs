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
//! # Status: fetch seam
//!
//! Today the crate carries the [`ImageFetcher`] trait and the default
//! `ReqwestFetcher` (feature `reqwest`). The view, loader and caches arrive
//! in a later phase.

pub mod fetch;
#[cfg(feature = "reqwest")]
mod reqwest_fetcher;

pub use fetch::*;
#[cfg(feature = "reqwest")]
pub use reqwest_fetcher::ReqwestFetcher;
