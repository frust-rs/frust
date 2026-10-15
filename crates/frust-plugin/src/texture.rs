//! The **producer** half of the external-texture frame bus: a plugin that
//! decodes or captures frames (a video player, a camera) pushes them into a
//! [`TextureSink`], a latest-frame mailbox, and whoever displays them takes
//! the newest one back out.
//!
//! # Why a plain mailbox, and who reads it
//!
//! This crate is a leaf: it names no renderer, no facade and no GPU type, and
//! the renderer never names it. So the sink is the whole of what a plugin
//! sees — plain data in, plain data out — and the consumer is a closure
//! written by whoever depends on *both* sides (a plugin's own facade-feature
//! half, or app code): it takes [`TextureSink::take_latest`]'s frame, moves
//! its fields into the renderer's own twin frame type (the field names match
//! one for one), and returns it to the renderer's frame source. Neither side
//! ever names the other.
//!
//! # Latest frame wins
//!
//! The mailbox holds at most one frame. A producer running ahead of the
//! display is the ordinary case (a decoder at 60 fps behind a 30 fps surface,
//! or a surface that stopped presenting), and queueing would only add latency
//! and memory: [`TextureSink::push`] replaces whatever was pending, and the
//! displaced frame's [`TextureFrame::release`] callback runs right there, on
//! the pushing thread. Every frame's callback therefore runs exactly once —
//! when it is displaced, when the consumer is done with it, or when the last
//! sink handle drops with it still pending.
//!
//! # Pixel contract
//!
//! [`TextureFormat::Rgba8`] frames are tightly packed rows (`width * 4` bytes
//! per row, no padding), top row first, in **premultiplied**-alpha RGBA — the
//! convention every paint in a frame travels in. A fully opaque frame reads
//! the same premultiplied or straight.

use std::ffi::c_void;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Process-unique counter behind [`TextureSink::id`].
static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);

/// The pixel layout of a [`TextureFrame`].
///
/// Non-exhaustive: `Nv12` (the bi-planar YUV layout hardware decoders emit)
/// is the reserved next variant, added once a consumer can convert it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextureFormat {
    /// 8-bit premultiplied RGBA, tightly packed (see the module docs).
    Rgba8,
}

/// Which platform object a [`NativeHandle`] points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeHandleKind {
    /// An Apple `CVPixelBufferRef` (AVFoundation, VideoToolbox, the camera).
    CvPixelBuffer,
    /// An Android `AHardwareBuffer*` (`MediaCodec`, `ImageReader`, CameraX).
    AHardwareBuffer,
}

/// An opaque, platform-owned GPU-shareable buffer handed through the sink
/// without being copied to the CPU.
///
/// This is a **sanctioned raw-pointer boundary**: the pointer is never
/// dereferenced by this crate, nor by the bus that carries it — only by an
/// importer for its [`NativeHandleKind`] on the consumer side. The plugin that
/// pushed it owns its lifetime: it holds its own reference to the buffer
/// (`CFRetain`, `AHardwareBuffer_acquire`) for as long as the frame is in
/// flight and drops that reference in the frame's [`TextureFrame::release`]
/// callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeHandle {
    /// The platform object `ptr` points at.
    pub kind: NativeHandleKind,
    /// The platform object itself (`CVPixelBufferRef`, `AHardwareBuffer*`).
    pub ptr: *mut c_void,
}

// SAFETY: a `NativeHandle` is an inert address plus a tag — nothing here
// dereferences it, and moving it between threads moves only the address. Both
// object kinds it may name are reference-counted platform objects documented
// as safe to retain, release and hand to the GPU from any thread
// (`CVPixelBuffer` is a CoreFoundation type; `AHardwareBuffer` is explicitly
// thread-safe), and the frame that carries the handle also carries the
// producer's own reference to the object (see the type docs), so the object
// stays alive for as long as the address travels.
unsafe impl Send for NativeHandle {}

/// Where a [`TextureFrame`]'s pixels live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramePayload {
    /// Pixels in CPU memory, laid out per the frame's [`TextureFormat`].
    /// Shared rather than owned, so a producer that keeps the buffer can hand
    /// it out without a copy.
    Cpu(Arc<[u8]>),
    /// A platform GPU buffer the consumer imports rather than uploads.
    Native(NativeHandle),
}

/// The callback a frame carries to tell its producer the consumer is done
/// with it.
pub type ReleaseCallback = Box<dyn FnOnce() + Send>;

/// One frame pushed through a [`TextureSink`].
///
/// Plain public fields, so the consumer-side conversion is a field-for-field
/// move into the renderer's own frame type. `release` is not run by dropping
/// the frame: whoever ends up holding a frame either moves the callback on or
/// calls [`Self::release`].
pub struct TextureFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How `payload`'s pixels are laid out.
    pub format: TextureFormat,
    /// The pixels, or the platform buffer holding them.
    pub payload: FramePayload,
    /// Run exactly once when the frame is no longer needed — displaced in the
    /// mailbox, consumed, or dropped with the last sink handle. `None` for a
    /// producer that needs no signal.
    pub release: Option<ReleaseCallback>,
}

impl TextureFrame {
    /// A CPU frame of tightly packed premultiplied RGBA8 with no release
    /// callback.
    #[must_use]
    pub fn rgba8(width: u32, height: u32, pixels: Arc<[u8]>) -> Self {
        Self {
            width,
            height,
            format: TextureFormat::Rgba8,
            payload: FramePayload::Cpu(pixels),
            release: None,
        }
    }

    /// Attaches the callback run once the frame is no longer needed,
    /// replacing any callback already attached (which is dropped unrun).
    #[must_use]
    pub fn on_release(mut self, release: impl FnOnce() + Send + 'static) -> Self {
        self.release = Some(Box::new(release));
        self
    }

    /// Runs the release callback, if any, consuming the frame — what a holder
    /// that is done with a frame without passing it on calls.
    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

impl fmt::Debug for TextureFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextureFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("payload", &self.payload)
            .field("release", &self.release.is_some())
            .finish()
    }
}

/// The mailbox shared by every clone of one [`TextureSink`].
struct Mailbox {
    id: u64,
    pending: Mutex<Option<TextureFrame>>,
}

impl Mailbox {
    /// The pending slot, with poison ignored: a panicking release callback
    /// runs outside the lock, so poison here could only be collateral, and
    /// one producer's panic must not wedge the display path.
    fn pending(&self) -> MutexGuard<'_, Option<TextureFrame>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        // The last handle is gone with a frame still pending: nobody will
        // take it, so its producer is told now.
        let pending = self
            .pending
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(frame) = pending {
            frame.release();
        }
    }
}

/// A latest-frame mailbox between a frame producer and its display. See the
/// [module docs](self).
///
/// A cheap `Arc` handle: clone it to give the producer and the consumer each
/// their own. `Send + Sync`, so the producer may push from a decoder or
/// capture thread while the consumer takes on the render thread.
#[derive(Clone)]
pub struct TextureSink {
    mailbox: Arc<Mailbox>,
}

impl TextureSink {
    /// An empty mailbox with a fresh [`Self::id`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            mailbox: Arc::new(Mailbox {
                id: NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed),
                pending: Mutex::new(None),
            }),
        }
    }

    /// A process-unique number naming this sink, shared by every clone.
    ///
    /// A plain counter for the producer's own bookkeeping (telling its sinks
    /// apart in a log or a map) — it is not the id a display list draws by;
    /// that one comes from the renderer when the consumer registers its
    /// frame source.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.mailbox.id
    }

    /// Makes `frame` the pending frame. A frame still pending from an earlier
    /// push is displaced and its release callback runs here, on the calling
    /// thread, after the mailbox lock is released.
    pub fn push(&self, frame: TextureFrame) {
        let displaced = self.mailbox.pending().replace(frame);
        if let Some(displaced) = displaced {
            displaced.release();
        }
    }

    /// Takes the pending frame, if any, leaving the mailbox empty. The caller
    /// now owns the frame's release callback.
    #[must_use]
    pub fn take_latest(&self) -> Option<TextureFrame> {
        self.mailbox.pending().take()
    }

    /// Whether a frame is pending.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.mailbox.pending().is_some()
    }
}

impl Default for TextureSink {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TextureSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextureSink")
            .field("id", &self.mailbox.id)
            .field("pending", &self.has_pending())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A 1x1 frame whose release bumps `counter`.
    fn counted(counter: &Arc<AtomicUsize>, value: u8) -> TextureFrame {
        let counter = Arc::clone(counter);
        TextureFrame::rgba8(1, 1, Arc::from(vec![value; 4])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn first_byte(frame: &TextureFrame) -> u8 {
        match &frame.payload {
            FramePayload::Cpu(pixels) => pixels[0],
            FramePayload::Native(_) => panic!("a CPU frame was pushed"),
        }
    }

    #[test]
    fn an_empty_sink_has_nothing_to_take() {
        let sink = TextureSink::new();
        assert!(!sink.has_pending());
        assert!(sink.take_latest().is_none());
    }

    #[test]
    fn take_latest_hands_back_the_pushed_frame_once() {
        let sink = TextureSink::new();
        sink.push(TextureFrame::rgba8(2, 3, Arc::from(vec![7u8; 24])));
        assert!(sink.has_pending());

        let frame = sink.take_latest().expect("the pushed frame is pending");
        assert_eq!((frame.width, frame.height), (2, 3));
        assert_eq!(frame.format, TextureFormat::Rgba8);
        assert_eq!(first_byte(&frame), 7);
        assert!(sink.take_latest().is_none(), "taking empties the mailbox");
    }

    #[test]
    fn the_newest_frame_wins_and_the_displaced_one_is_released() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();

        sink.push(counted(&released, 1));
        sink.push(counted(&released, 2));
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "the displaced frame was released on push"
        );

        let frame = sink.take_latest().expect("a frame is pending");
        assert_eq!(first_byte(&frame), 2, "the newest frame wins");
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "taking a frame does not release it — the taker owns that now"
        );

        frame.release();
        assert_eq!(released.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn release_runs_exactly_once_per_frame() {
        let released = Arc::new(AtomicUsize::new(0));
        let frame = counted(&released, 0);
        frame.release();
        assert_eq!(released.load(Ordering::SeqCst), 1);

        // A frame with no callback releases to nothing, without complaint.
        TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).release();
    }

    #[test]
    fn dropping_the_last_handle_releases_a_pending_frame() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();
        let consumer = sink.clone();
        sink.push(counted(&released, 0));

        drop(sink);
        assert_eq!(
            released.load(Ordering::SeqCst),
            0,
            "a clone still holds the mailbox, so the frame stays pending"
        );
        drop(consumer);
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "the last handle took the pending frame's release with it"
        );
    }

    #[test]
    fn clones_share_one_mailbox_and_one_id() {
        let producer = TextureSink::new();
        let consumer = producer.clone();
        assert_eq!(producer.id(), consumer.id());

        producer.push(TextureFrame::rgba8(1, 1, Arc::from(vec![9u8; 4])));
        let frame = consumer.take_latest().expect("the clone sees the push");
        assert_eq!(first_byte(&frame), 9);
    }

    #[test]
    fn sink_ids_are_process_unique() {
        let a = TextureSink::new();
        let b = TextureSink::new();
        let c = TextureSink::default();
        assert_ne!(a.id(), b.id());
        assert_ne!(b.id(), c.id());
        assert_ne!(a.id(), c.id());
    }

    #[test]
    fn sink_ids_are_unique_across_threads() {
        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| TextureSink::new().id()))
            .collect();
        let mut ids: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8);
    }

    #[test]
    fn a_producer_thread_and_a_consumer_release_every_frame_once() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();
        let producer = {
            let sink = sink.clone();
            let released = Arc::clone(&released);
            std::thread::spawn(move || {
                for value in 0..200u8 {
                    sink.push(counted(&released, value));
                }
            })
        };
        let mut taken = 0usize;
        while !producer.is_finished() || sink.has_pending() {
            if let Some(frame) = sink.take_latest() {
                taken += 1;
                frame.release();
            }
        }
        producer.join().unwrap();
        if let Some(frame) = sink.take_latest() {
            taken += 1;
            frame.release();
        }
        assert!(taken >= 1, "the consumer saw at least the last frame");
        assert_eq!(
            released.load(Ordering::SeqCst),
            200,
            "every pushed frame was released exactly once, displaced or taken"
        );
    }

    #[test]
    fn a_native_handle_travels_untouched() {
        let mut byte = 0u8;
        let ptr: *mut c_void = (&raw mut byte).cast();
        let sink = TextureSink::new();
        sink.push(TextureFrame {
            width: 4,
            height: 4,
            format: TextureFormat::Rgba8,
            payload: FramePayload::Native(NativeHandle {
                kind: NativeHandleKind::AHardwareBuffer,
                ptr,
            }),
            release: None,
        });

        // Taken on another thread, as a render thread would.
        let frame = std::thread::spawn(move || sink.take_latest())
            .join()
            .unwrap()
            .expect("the native frame is pending");
        match frame.payload {
            FramePayload::Native(handle) => {
                assert_eq!(handle.kind, NativeHandleKind::AHardwareBuffer);
                assert_eq!(handle.ptr, ptr, "the address is carried, never touched");
            }
            FramePayload::Cpu(_) => panic!("a native frame was pushed"),
        }
    }

    #[test]
    fn a_frame_debug_reports_whether_a_release_is_attached() {
        let frame = TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(|| {});
        let text = format!("{frame:?}");
        assert!(text.contains("release: true"), "{text}");
    }
}
