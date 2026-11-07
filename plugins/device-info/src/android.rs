//! Android backend (`ActivityManager.getMemoryInfo`, `isLowRamDevice`) over plain JNI through
//! `frust-plugin`. Stub: filled by card dm-02.

use crate::DeviceMemory;

pub(crate) fn query() -> Option<DeviceMemory> {
    // stub: filled by dm-02
    None
}
