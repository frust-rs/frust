//! The texture-output half of [`VideoPlayerHandle`]: the bridge from the
//! session's [`TextureSink`] (the plugin substrate's latest-frame mailbox the
//! backend pushes decoded frames into) to a renderer scene texture that
//! `frust::texture_view` draws in-scene.
//!
//! The renderer and the plugin substrate never name each other, so the bridge
//! is a closure written here, where both are in reach: each frame the
//! renderer polls it, it takes the newest frame out of the sink and carries
//! its fields into the renderer's own twin type (`frust::texture::SourceFrame`).
//! The frame's release callback travels with it, so the producer learns when
//! the renderer is done with each frame — see `frust::texture`'s module doc
//! for when that is.

use frust::texture::{
    NativeHandle, NativeHandleKind, SourceFormat, SourceFrame, SourcePayload, TextureRegistration,
    register_texture_source,
};
use frust_plugin::texture::{
    FramePayload, NativeHandleKind as FrameHandleKind, TextureFormat, TextureFrame, TextureSink,
};

use super::handle::VideoPlayerHandle;

impl VideoPlayerHandle {
    /// The session's frame mailbox — [`crate::PlayerSession::texture_sink`]:
    /// `Some` for a session opened with [`crate::VideoOutput::Texture`] whose
    /// backend has a frame producer on this target, `None` otherwise.
    pub fn texture_sink(&self) -> Option<TextureSink> {
        self.session().texture_sink()
    }

    /// The scene texture this session's frames are drawn as, registered with
    /// the renderer on the first call and kept on this handle from then on
    /// (dropping the handle unregisters it). `None` when the session has no
    /// [`Self::texture_sink`].
    ///
    /// Feed [`TextureRegistration::scene_texture_id`] to `frust::texture_view`
    /// — or use [`super::video_texture_view`], which does that and the fit.
    /// The registration takes every frame out of the sink, so nothing else
    /// should take from [`Self::texture_sink`] once this was called.
    pub fn texture_output(&self) -> Option<&TextureRegistration> {
        self.texture_registration()
            .get_or_init(|| self.texture_sink().map(register_sink))
            .as_ref()
    }
}

/// Registers `sink` as a renderer frame source: each poll takes its newest
/// frame and converts it with [`source_frame`].
fn register_sink(sink: TextureSink) -> TextureRegistration {
    register_texture_source(move || sink.take_latest().and_then(source_frame))
}

/// Carries a producer-side frame into the renderer's twin type, field for
/// field, its release callback moved across. A frame in a pixel format the
/// renderer does not know is released and skipped (the last good frame stays
/// shown).
fn source_frame(frame: TextureFrame) -> Option<SourceFrame> {
    // Consumed by value: the producer-side handle moves into the one renderer
    // frame built below, and the drop guard of `frame` is disarmed.
    let (meta, payload, release) = frame.into_parts();
    let format = match meta.format {
        TextureFormat::Rgba8 => SourceFormat::Rgba8,
        // `TextureFormat` is non-exhaustive; a layout added there before the
        // renderer can show it is handed straight back.
        _ => {
            if let Some(release) = release {
                release();
            }
            return None;
        }
    };
    let payload = match payload {
        FramePayload::Cpu(pixels) => SourcePayload::Cpu(pixels),
        FramePayload::Native(handle) => {
            let kind = match handle.kind() {
                FrameHandleKind::CvPixelBuffer => NativeHandleKind::CvPixelBuffer,
                FrameHandleKind::AHardwareBuffer => NativeHandleKind::AHardwareBuffer,
            };
            // SAFETY: forwards the producer's guarantee. The substrate's
            // handle exists only through its own `unsafe` constructor, whose
            // contract is this one's word for word: a live object of this
            // kind, retained by the producer until the release callback of
            // the one frame carrying it has run. That handle is move-only and
            // was consumed by value above, so no other copy of the address
            // exists, and the callback moves across with it below.
            SourcePayload::Native(unsafe { NativeHandle::new(kind, handle.ptr()) })
        }
    };
    Some(SourceFrame {
        width: meta.width,
        height: meta.height,
        format,
        payload,
        release,
    })
}

/// Whether the renderer can import Android hardware buffers right now — what
/// the Android backend asks when a texture session opens, to choose between
/// handing over `AHardwareBuffer`s and copying frames into CPU memory.
#[cfg(target_os = "android")]
pub(crate) fn hardware_buffer_import_installed() -> bool {
    frust::texture::native_importer_installed(NativeHandleKind::AHardwareBuffer)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{PlayerOptions, VideoOutput, VideoPlayer, VideoSource};

    fn open(output: VideoOutput) -> VideoPlayerHandle {
        let session = VideoPlayer::open(
            VideoSource::Url("https://example.invalid/clip.mp4".to_owned()),
            PlayerOptions {
                output,
                ..PlayerOptions::default()
            },
        )
        .expect("the mock backend opens an ordinary source");
        VideoPlayerHandle::new(session)
    }

    #[test]
    fn a_native_view_handle_has_no_texture_output() {
        let handle = open(VideoOutput::NativeView);
        assert!(handle.texture_sink().is_none());
        assert!(handle.texture_output().is_none());
    }

    #[test]
    fn a_texture_handle_registers_one_scene_texture_and_keeps_it() {
        let handle = open(VideoOutput::Texture);
        let first = handle
            .texture_output()
            .expect("a texture session registers")
            .scene_texture_id();
        let second = handle
            .texture_output()
            .expect("still registered")
            .scene_texture_id();
        assert_eq!(first, second, "registered once, not per call");
        assert!(
            handle
                .texture_output()
                .is_some_and(|r| r.frame_size().is_none())
        );
    }

    #[test]
    fn a_cpu_frame_converts_field_for_field_with_its_release() {
        let released = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&released);
        let frame = TextureFrame::rgba8(3, 2, Arc::from(vec![9u8; 24])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        let converted = source_frame(frame).expect("an RGBA frame converts");
        assert_eq!((converted.width, converted.height), (3, 2));
        assert_eq!(converted.format, SourceFormat::Rgba8);
        assert!(matches!(&converted.payload, SourcePayload::Cpu(p) if p.len() == 24));
        assert_eq!(released.load(Ordering::SeqCst), 0);
        converted.release();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_native_frame_keeps_its_handle_kind_and_pointer() {
        let mut marker = 0u8;
        let ptr = std::ptr::from_mut(&mut marker).cast();
        // SAFETY: the converted handle is compared, never imported.
        let handle = unsafe {
            frust_plugin::texture::NativeHandle::new(FrameHandleKind::AHardwareBuffer, ptr)
        };
        let frame = TextureFrame {
            width: 8,
            height: 8,
            format: TextureFormat::Rgba8,
            payload: FramePayload::Native(handle),
            release: None,
        };

        let converted = source_frame(frame).expect("a native frame converts");
        let SourcePayload::Native(converted) = &converted.payload else {
            panic!("a native frame stays native");
        };
        assert_eq!(converted.kind(), NativeHandleKind::AHardwareBuffer);
        assert_eq!(converted.ptr(), ptr);
    }

    #[test]
    fn a_converted_frame_releases_once_through_the_renderer_twin() {
        let released = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&released);
        let frame = TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let converted = source_frame(frame).expect("an RGBA frame converts");
        assert_eq!(
            released.load(Ordering::SeqCst),
            0,
            "the callback moved across; the emptied producer frame released nothing"
        );
        drop(converted);
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }
}
