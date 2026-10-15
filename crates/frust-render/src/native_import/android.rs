//! Android importer: wraps `frust_gpu::import::android::import_hardware_buffer`
//! as the bus's `NativeImporter` for `NativeHandleKind::AHardwareBuffer`, installed
//! only when the device was opened with hardware-buffer import support.
//! Not yet implemented: the Android importer lands here.

/// Install the Android hardware-buffer importer when the device supports it.
pub fn install(_context: &frust_gpu::RenderContext) {}
