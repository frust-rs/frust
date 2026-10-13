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
//! # Status: pin gate only
//!
//! Today the crate carries only its dependency rows, proving the HTTP/TLS
//! stack compiles for every target. The view, loader and caches arrive in a
//! later phase; [`pin_gate_marker`] is a placeholder until then.

/// Placeholder proving the crate links on every target; removed when the
/// real modules land.
pub fn pin_gate_marker() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_gate_marker_is_callable() {
        pin_gate_marker();
    }
}
