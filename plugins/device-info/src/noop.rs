//! Web and every other target: nothing to report. `navigator.deviceMemory` is Chromium-only,
//! coarse and secure-context-gated, so v1 reports nothing on web.

use crate::DeviceMemory;

pub(crate) fn query() -> Option<DeviceMemory> {
    None
}
