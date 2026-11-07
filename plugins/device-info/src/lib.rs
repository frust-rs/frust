//! Device memory query for Frust apps.
//!
//! [`DeviceMemory::query`] answers "how much memory does this device have, and how much is
//! left?" so an app can size an image or data working set.
//!
//! # Why it exists
//!
//! On Android 14 and later a foreground app receives no `onTrimMemory` level:
//! `RUNNING_MODERATE`, `RUNNING_LOW` and `RUNNING_CRITICAL` are not delivered ("not notified since
//! API level 34"), only `UI_HIDDEN` and `BACKGROUND` remain, and `onLowMemory` is never called. The
//! framework's `on_memory_pressure` event therefore never fires while an app is on screen, and an
//! app has no signal to size a cache by. This crate gives it a figure to poll instead.
//!
//! # Per platform
//!
//! | Platform | `total_bytes` | `system_available_bytes` | `process_headroom_bytes` | `system_low` |
//! |---|---|---|---|---|
//! | Android | `MemoryInfo.totalMem` | `MemoryInfo.availMem` | none | `MemoryInfo.lowMemory` |
//! | iOS | `sysctl hw.memsize` | none | `os_proc_available_memory()` | none |
//! | macOS | `sysctl hw.memsize` | none | none | none |
//! | Linux | `/proc/meminfo` `MemTotal` | `MemAvailable` | none | none |
//! | Windows | `ullTotalPhys` | `ullAvailPhys` | none | none |
//! | Web, other | `query()` is `None` | | | |
//!
//! # Usage
//!
//! The call is synchronous and cheap, may be made from any thread, and needs no setup beyond
//! the shell's normal platform initialisation. It is a reading, not a signal: nothing is cached,
//! so re-query when you need a fresh figure, for example from
//! `AppLifecycleListener::on_memory_pressure` or when a page opens.
//!
//! ```ignore
//! use frust_device_info::DeviceMemory;
//!
//! if let Some(memory) = DeviceMemory::query() {
//!     let gib = memory.total_bytes as f64 / (1u64 << 30) as f64;
//!     println!("{gib:.1} GiB total, low-RAM device: {}", memory.low_ram_device);
//! }
//! ```

mod meminfo;

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
)))]
mod noop;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "android")]
use android as platform;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use apple as platform;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "linux",
    target_os = "windows"
)))]
use noop as platform;
#[cfg(target_os = "windows")]
use windows as platform;

/// One reading of the device's memory. Fields a platform cannot report are `None` (or `false`
/// for [`low_ram_device`](Self::low_ram_device)).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceMemory {
    /// Total physical RAM in bytes. Android `MemoryInfo.totalMem`, Apple `sysctl hw.memsize`,
    /// Linux `MemTotal`, Windows `ullTotalPhys`.
    pub total_bytes: u64,
    /// Android `ActivityManager.isLowRamDevice()`, generally a device with 1 GB of RAM or less.
    /// `false` on every other platform.
    pub low_ram_device: bool,
    /// Memory the system reports as available, in bytes. Android `MemoryInfo.availMem` (a
    /// system-wide estimate that "should not be considered absolute", not this process's
    /// budget), Linux `MemAvailable`, Windows `ullAvailPhys`. `None` on Apple and web.
    pub system_available_bytes: Option<u64>,
    /// Bytes this app may still allocate before the OS limit: iOS `os_proc_available_memory()`
    /// only. Advisory and moving; never cache it. `None` elsewhere.
    pub process_headroom_bytes: Option<u64>,
    /// Android `MemoryInfo.lowMemory`: the system is currently below its low-memory threshold.
    /// This is the one live low-memory state a foreground app still gets on API 34 and later.
    /// `None` elsewhere.
    pub system_low: Option<bool>,
}

impl DeviceMemory {
    /// Reads the device's memory now.
    ///
    /// Returns `None` when the platform cannot report: web and other targets, Android before the
    /// shell's `nativeInitPlatform` has installed the platform handles
    /// (`frust_plugin::PlatformHandleError::NotInitialized`), or a JNI or syscall failure
    /// (logged at `debug`).
    #[must_use]
    pub fn query() -> Option<DeviceMemory> {
        platform::query()
    }
}
