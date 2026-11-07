//! iOS and macOS backend.
//!
//! Two reads, neither cached (the figures are cheap and the headroom moves):
//!
//! - `sysctl hw.memsize` on both platforms: the device's physical memory. On a 4 GB-class
//!   iPhone this reads below 4 GiB, because the kernel reserves part of the RAM for itself
//!   (`crates/frust-engine/src/cache/images.rs` notes the same).
//! - `os_proc_available_memory()` on iOS only: the bytes this app may still allocate before it
//!   hits its memory limit. It needs iOS 13, which is below frust's iOS floor. A frust binary on
//!   iOS is always an app, so `0` is a real reading: the headroom is exhausted.
//!
//! macOS reports no headroom and no system-available figure in v1: `os_proc_available_memory`
//! is `API_UNAVAILABLE` there, macOS has no per-process memory limit API, and the Mach
//! `host_statistics64` read is a later card.

use crate::DeviceMemory;

/// The device's physical memory in bytes from `sysctl hw.memsize`, or `None` when the read
/// fails or reports nothing.
fn physical_memory() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: `c"hw.memsize"` is NUL-terminated; `bytes` is a valid, writable `u64` and `len`
    // holds its size, so the kernel writes at most 8 bytes into it; the new-value pointer is
    // null with length 0 (a read).
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast::<libc::c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && len == std::mem::size_of::<u64>() && bytes > 0).then_some(bytes)
}

// `libc` does not export `os_proc_available_memory`, so it is declared here (it lives in
// libSystem, which every Apple target already links).
#[cfg(target_os = "ios")]
unsafe extern "C" {
    /// Bytes the app may allocate before it hits its memory limit; 0 if the calling process is
    /// not an app or has already exceeded its limit. Advisory; do not cache. iOS 13+.
    fn os_proc_available_memory() -> usize;
}

/// Remaining allocation headroom of this app. `Some(0)` is a reading, not an absence: on iOS
/// the calling process is always an app, so 0 means the app is at or over its memory limit.
#[cfg(target_os = "ios")]
fn headroom() -> Option<u64> {
    // SAFETY: `os_proc_available_memory` takes no arguments, reads only process accounting
    // state and has no preconditions.
    let n = unsafe { os_proc_available_memory() };
    Some(n as u64)
}

/// macOS has no headroom: `os_proc_available_memory` is `API_UNAVAILABLE` there and the OS
/// offers no per-process memory limit to report.
#[cfg(target_os = "macos")]
fn headroom() -> Option<u64> {
    None
}

pub(crate) fn query() -> Option<DeviceMemory> {
    Some(DeviceMemory {
        total_bytes: physical_memory()?,
        low_ram_device: false,
        system_available_bytes: None,
        process_headroom_bytes: headroom(),
        system_low: None,
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn macos_reports_total_only() {
        let m = DeviceMemory::query().expect("macOS can read hw.memsize");
        assert!(m.total_bytes > 1 << 30);
        assert!(m.process_headroom_bytes.is_none());
        assert!(m.system_available_bytes.is_none());
        assert!(query().is_some());
    }
}
