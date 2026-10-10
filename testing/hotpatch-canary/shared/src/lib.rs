//! The hot-patch canary app's local path dependency outside its workspace,
//! shaped like a `--frust-path` checkout's crate: cargo builds it without
//! `RUSTC_WORKSPACE_WRAPPER`, so the builder captures it through
//! `RUSTC_WRAPPER`. The driver edits this file while the app runs (the
//! [`offset`] value, then a field added to [`Offset`]) and restores it
//! afterwards.

/// The value the app's hot function reads from this crate and returns
/// inside its own `Reading`, by value: its layout is what the builder's L3
/// gate must cover for a non-member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offset {
    pub value: u64,
}

/// The function the app's hot function calls across the crate boundary.
#[inline(never)]
pub fn offset() -> Offset {
    Offset { value: 0 }
}
