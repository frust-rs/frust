//! iOS and macOS backend (`sysctl hw.memsize`, iOS `os_proc_available_memory`). Stub: filled by
//! card dm-03.

use crate::DeviceMemory;

pub(crate) fn query() -> Option<DeviceMemory> {
    // stub: filled by dm-03
    None
}
