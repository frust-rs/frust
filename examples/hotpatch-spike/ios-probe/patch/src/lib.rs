//! The library the iOS simulator load probe copies into an installed Frust app's data container
//! and loads into the running app.
//!
//! It exports one C symbol with a known return value, so the probe app can tell a real load and
//! symbol resolution apart from a stale or missing file.

/// The value the probe app expects back after resolving this symbol.
#[unsafe(no_mangle)]
pub extern "C" fn frust_probe_value() -> u32 {
    42
}
