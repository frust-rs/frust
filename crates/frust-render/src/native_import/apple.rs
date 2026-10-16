//! Apple importer: wraps `frust_gpu::import::apple::MetalImporter` as the bus's
//! `NativeImporter` for `NativeHandleKind::CvPixelBuffer`, installed once the
//! render context has a device.

use std::sync::{Arc, Mutex, Once, PoisonError};

use frust_gpu::RenderContext;
use frust_gpu::import::apple::{MetalImportError, MetalImporter};

use crate::texture_bus::{
    ImportError, ImportedTexture, NativeHandle, NativeHandleKind, NativeImporter,
    set_native_importer,
};

/// Imports `CVPixelBuffer` handles through the frame-bus importer seam.
///
/// A `MetalImporter` owns a texture cache tied to one device, so this keeps
/// the one for the device it was last handed and builds a fresh one when the
/// bus hands it another (a shell that reopened its device).
struct AppleImporter {
    importer: Mutex<Option<MetalImporter>>,
}

impl NativeImporter for AppleImporter {
    fn import(
        &self,
        handle: &NativeHandle,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Result<ImportedTexture, ImportError> {
        let mut slot = self.importer.lock().unwrap_or_else(PoisonError::into_inner);
        let importer = match slot.take() {
            Some(importer) if importer.device() == device => importer,
            _ => MetalImporter::new(device).map_err(to_import_error)?,
        };
        let importer = slot.insert(importer);
        // SAFETY: `NativeHandle::new`'s contract, the only way to build a
        // handle: a live `CVPixelBufferRef` (this importer is installed for
        // that kind alone, and the handle's kind was matched to it) on which
        // the producer holds a retained reference until the frame's release
        // callback runs, which the bus does only after this call returns — so
        // the buffer is live for the whole call. The producer finished the
        // frame before handing it over and does not write it again until that
        // release, which the bus holds until the imported texture is retired
        // and the GPU finished with it. The importer takes its own retain for
        // the texture's lifetime.
        let (texture, size) = unsafe { importer.import(handle.ptr()) }.map_err(to_import_error)?;
        Ok(ImportedTexture {
            view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
            size,
        })
    }
}

fn to_import_error(error: MetalImportError) -> ImportError {
    match error {
        MetalImportError::Unsupported(reason) => ImportError::Unsupported(reason),
        other => ImportError::Failed(other.to_string()),
    }
}

/// Install the Apple CVPixelBuffer importer once the context has a device.
///
/// Safe to call every frame: nothing is installed before the device exists,
/// and the installation happens at most once per process.
pub fn install(context: &RenderContext) {
    static INSTALLED: Once = Once::new();
    if context.caps().is_none() {
        return;
    }
    INSTALLED.call_once(|| {
        set_native_importer(
            NativeHandleKind::CvPixelBuffer,
            Arc::new(AppleImporter {
                importer: Mutex::new(None),
            }),
        );
    });
}
