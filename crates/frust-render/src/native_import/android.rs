//! Android importer: wraps `frust_gpu::import::android::import_hardware_buffer`
//! as the bus's `NativeImporter` for `NativeHandleKind::AHardwareBuffer`, installed
//! only when the device was opened with hardware-buffer import support.

use std::sync::{Arc, Once};

use frust_gpu::RenderContext;

use crate::texture_bus::{
    ImportError, ImportedTexture, NativeHandle, NativeHandleKind, NativeImporter,
    set_native_importer,
};

/// Imports `AHardwareBuffer` handles through the frame-bus importer seam.
struct AndroidImporter;

impl NativeImporter for AndroidImporter {
    fn import(
        &self,
        handle: &NativeHandle,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Result<ImportedTexture, ImportError> {
        // SAFETY: the pointer — `NativeHandle::new`'s contract, the only way
        // to build a handle: a live `AHardwareBuffer*` (this importer is
        // installed for that kind alone, and the handle's kind was matched to
        // it) on which the producer holds a reference until the frame's
        // release callback runs, which the bus does only after this call
        // returns — so the buffer is live for the whole call. The queue: the
        // bus calls `import` from `record()` on the render thread, which
        // records, submits and presents the frame, so the importer's queue
        // submission cannot overlap a `wgpu` submit from that thread; a
        // `wgpu` submitter on another thread is unsupported while the import
        // is on (frust-gpu's module docs; a debug build asserts the importing
        // thread). The importer takes its own buffer reference, so the
        // producer may release its own as soon as this call returns.
        let imported = unsafe {
            frust_gpu::import::android::import_hardware_buffer(device, handle.ptr().cast())
        };
        let texture = imported.map_err(|error| match error {
            frust_gpu::import::android::HardwareBufferImportError::Unsupported(reason) => {
                ImportError::Unsupported(reason)
            }
            other => ImportError::Failed(other.to_string()),
        })?;
        Ok(ImportedTexture {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            size: (texture.width(), texture.height()),
        })
    }
}

/// Install the Android hardware-buffer importer when the device supports it.
///
/// Safe to call every frame: nothing is installed until the live device reports
/// `hwbuffer_import`, and the installation happens at most once per process.
pub fn install(context: &RenderContext) {
    static INSTALLED: Once = Once::new();
    if !context.caps().is_some_and(|caps| caps.hwbuffer_import) {
        return;
    }
    INSTALLED.call_once(|| {
        set_native_importer(NativeHandleKind::AHardwareBuffer, Arc::new(AndroidImporter));
    });
}
