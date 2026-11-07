//! Windows backend: `GlobalMemoryStatusEx`. Called on every call; nothing is cached.

use crate::DeviceMemory;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

pub(crate) fn query() -> Option<DeviceMemory> {
    // SAFETY: `MEMORYSTATUSEX` is plain integers, so all-zero is a valid value.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a live, exclusively borrowed `MEMORYSTATUSEX` with `dwLength` set
    // as the API requires; the call only writes within it.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        log::debug!("frust-device-info: GlobalMemoryStatusEx failed");
        return None;
    }
    Some(DeviceMemory {
        total_bytes: status.ullTotalPhys,
        low_ram_device: false,
        system_available_bytes: Some(status.ullAvailPhys),
        process_headroom_bytes: None,
        system_low: None,
    })
}
