//! The `frust-api` half of the preview texture: the bridge from a session's
//! [`TextureSink`] (the plugin substrate's latest-frame mailbox,
//! [`CameraSession::preview_texture`]) to a renderer scene texture that
//! `frust::texture_view` draws in-scene. The only module in this crate that
//! names `frust`.
//!
//! The renderer and the plugin substrate never name each other, so the bridge
//! is a closure written here, where both are in reach: each frame the renderer
//! polls it, it takes the newest frame out of the sink and moves its fields
//! into the renderer's own twin type (`frust::texture::SourceFrame`), release
//! callback included.

use std::sync::OnceLock;

use frust::texture::{
    NativeHandle, NativeHandleKind, SourceFormat, SourceFrame, SourcePayload, TextureRegistration,
    register_texture_source,
};
use frust_plugin::texture::{
    FramePayload, NativeHandleKind as FrameHandleKind, TextureFormat, TextureFrame, TextureSink,
};

use crate::CameraSession;

/// Where a session keeps the scene texture `preview_texture_output`
/// registered.
pub(crate) type OutputSlot = OnceLock<Option<TextureRegistration>>;

impl CameraSession {
    /// The live preview as a scene texture: [`Self::preview_texture`]'s
    /// mailbox registered with the renderer on the first call and kept on this
    /// session from then on (dropping the session unregisters it). `None`
    /// when [`Self::preview_texture`] is.
    ///
    /// Feed [`TextureRegistration::scene_texture_id`] to
    /// `frust::texture_view`; [`TextureRegistration::frame_size`] reports the
    /// shown frame's extent for its `.source_size(..)`. The registration takes
    /// every frame out of the mailbox, so nothing else should take from
    /// [`Self::preview_texture`] once this was called.
    pub fn preview_texture_output(&self) -> Option<&TextureRegistration> {
        self.texture_output
            .get_or_init(|| self.preview_texture().map(register_sink))
            .as_ref()
    }
}

/// Registers `sink` as a renderer frame source: each poll takes its newest
/// frame and converts it with [`source_frame`].
fn register_sink(sink: TextureSink) -> TextureRegistration {
    register_texture_source(move || sink.take_latest().and_then(source_frame))
}

/// Moves a producer-side frame into the renderer's twin type, field for
/// field. A frame in a pixel format the renderer does not know is released
/// and skipped (the last good frame stays shown).
fn source_frame(frame: TextureFrame) -> Option<SourceFrame> {
    let format = match frame.format {
        TextureFormat::Rgba8 => SourceFormat::Rgba8,
        // `TextureFormat` is non-exhaustive; a layout added there before the
        // renderer can show it is handed straight back.
        _ => {
            frame.release();
            return None;
        }
    };
    let payload = match frame.payload {
        FramePayload::Cpu(pixels) => SourcePayload::Cpu(pixels),
        FramePayload::Native(handle) => SourcePayload::Native(NativeHandle {
            kind: match handle.kind {
                FrameHandleKind::CvPixelBuffer => NativeHandleKind::CvPixelBuffer,
                FrameHandleKind::AHardwareBuffer => NativeHandleKind::AHardwareBuffer,
            },
            ptr: handle.ptr,
        }),
    };
    Some(SourceFrame {
        width: frame.width,
        height: frame.height,
        format,
        payload,
        release: frame.release,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn a_cpu_frame_converts_field_for_field_with_its_release() {
        let released = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&released);
        let frame = TextureFrame::rgba8(4, 3, Arc::from(vec![5u8; 48])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        let converted = source_frame(frame).expect("an RGBA frame converts");
        assert_eq!((converted.width, converted.height), (4, 3));
        assert_eq!(converted.format, SourceFormat::Rgba8);
        assert!(matches!(&converted.payload, SourcePayload::Cpu(p) if p.len() == 48));
        converted.release();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_registered_sink_hands_its_frames_to_the_renderer_source() {
        let sink = TextureSink::new();
        let registration = register_sink(sink.clone());
        // Nothing shown yet: the renderer has not polled.
        assert_eq!(registration.frame_size(), None);
        sink.push(TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])));
        assert!(sink.has_pending(), "the renderer takes on its own frame");
    }
}
