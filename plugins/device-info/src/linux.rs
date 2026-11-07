//! Linux backend: `/proc/meminfo`. Read on every call; the available figure moves, so nothing
//! is cached.

use crate::{DeviceMemory, meminfo};

pub(crate) fn query() -> Option<DeviceMemory> {
    let text = match std::fs::read_to_string("/proc/meminfo") {
        Ok(text) => text,
        Err(err) => {
            log::debug!("frust-device-info: reading /proc/meminfo failed: {err}");
            return None;
        }
    };
    let info = meminfo::parse(&text)?;
    Some(DeviceMemory {
        total_bytes: info.total_bytes,
        low_ram_device: false,
        system_available_bytes: info.available_bytes,
        process_headroom_bytes: None,
        system_low: None,
    })
}
