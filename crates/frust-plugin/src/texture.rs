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
//! half, or app code): it takes [`TextureSink::take_latest`]'s frame, carries
//! its parts across into the renderer's own twin frame type (the extent and
//! format match one for one; the payload and release callback travel together
//! through [`TextureFrame::into_parts`]), and returns it to the renderer's
//! frame source. Neither side ever names the other.
//!
//! # Latest frame wins
//!
//! The mailbox holds at most one frame. A producer running ahead of the
//! display is the ordinary case (a decoder at 60 fps behind a 30 fps surface,
//! or a surface that stopped presenting), and queueing would only add latency
//! and memory: [`TextureSink::push`] replaces whatever was pending, and the
//! displaced frame's [`TextureFrame::release`] callback runs right there, on
//! the pushing thread. Every frame's callback therefore runs exactly once —
//! when it is displaced, when the consumer is done with it, when the last
//! sink handle drops with it still pending, or when a frame still holding its
//! callback is dropped (a taker that forgot it, a panic unwinding past it).
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
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

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
///
/// The fields are private and the only constructor is the `unsafe`
/// [`Self::new`]: the importer that eventually dereferences the pointer
/// relies on nothing but that constructor's contract, so safe code cannot
/// forge a handle.
///
/// A handle is move-only — neither `Clone` nor `Copy` — so it cannot be
/// copied, and a frame's payload and release callback are private to the
/// frame: outside `unsafe`, a handle cannot leave the frame that carries its
/// release. Reading it in place goes through [`TextureFrame::payload`]; the one
/// way to take it out is the `unsafe` [`TextureFrame::into_parts`], whose
/// caller promises to run the returned callback once, after the handle's last
/// use.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeHandle {
    /// The platform object `ptr` points at.
    kind: NativeHandleKind,
    /// The platform object itself (`CVPixelBufferRef`, `AHardwareBuffer*`).
    ptr: *mut c_void,
}

impl NativeHandle {
    /// Wraps `ptr`, a platform object of `kind`, for a
    /// [`FramePayload::Native`] frame.
    ///
    /// # Safety
    ///
    /// `ptr` is a live platform object of exactly `kind` — a
    /// `CVPixelBufferRef` for [`NativeHandleKind::CvPixelBuffer`], an
    /// `AHardwareBuffer*` for [`NativeHandleKind::AHardwareBuffer`] — on which
    /// the caller holds a retained reference of its own (`CFRetain`,
    /// `AHardwareBuffer_acquire`), and the caller keeps that reference until
    /// the release callback of the one frame this handle is placed in has run.
    /// The handle is move-only, so it cannot be copied, and outside `unsafe`
    /// it cannot leave the frame that carries it with that callback
    /// ([`TextureFrame::into_parts`] is `unsafe` for exactly that reason). The
    /// consumer-side importer for `kind` dereferences the pointer (describes,
    /// imports and retains the object) on this promise alone, so a dangling,
    /// foreign or mistyped pointer is undefined behaviour there.
    #[must_use]
    pub unsafe fn new(kind: NativeHandleKind, ptr: *mut c_void) -> Self {
        Self { kind, ptr }
    }

    /// The platform object [`Self::ptr`] points at.
    #[must_use]
    pub fn kind(&self) -> NativeHandleKind {
        self.kind
    }

    /// The platform object itself, as handed to [`Self::new`]. Only the handle
    /// is move-only: an address copied out of it carries no lifetime guarantee
    /// past the frame's release, and using it needs the caller's own `unsafe`.
    #[must_use]
    pub fn ptr(&self) -> *mut c_void {
        self.ptr
    }
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
///
/// Not `Clone`: a native payload holds a move-only [`NativeHandle`].
#[derive(Debug, PartialEq, Eq)]
pub enum FramePayload {
    /// Pixels in CPU memory, laid out per the frame's [`TextureFormat`].
    /// Shared rather than owned, so a producer that keeps the buffer can hand
    /// it out without a copy.
    Cpu(Arc<[u8]>),
    /// A platform GPU buffer the consumer imports rather than uploads.
    Native(NativeHandle),
}

/// The callback a frame carries to tell its producer the consumer is done
/// with it. It runs from a `Drop` and must not panic; see [`run_release`] for
/// what happens if it does anyway.
pub type ReleaseCallback = Box<dyn FnOnce() + Send>;

/// Runs a frame's release callback behind `catch_unwind`, the last line of
/// defence for a callback that panics anyway.
///
/// Releases run from `Drop` impls, sometimes while another panic unwinds, and
/// a panic escaping a `Drop` during an unwind aborts the process. The guard
/// only works in panic=unwind builds (dev and test): the shipped release
/// profile sets `panic = "abort"`, where a panicking callback aborts the
/// process and the must-not-panic rule is the only protection. The panic hook
/// has already reported the message by the time it is caught here, so the
/// payload is discarded and nothing more is logged on this side (this crate
/// has no logging dependency).
///
/// # Panics
///
/// Never in an unwinding build; a callback must still not panic, since a
/// caught one is a bug in the producer.
fn run_release(release: ReleaseCallback) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(release));
}

/// The extent and pixel format of a [`TextureFrame`], as returned by
/// [`TextureFrame::into_parts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMeta {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How the payload's pixels are laid out.
    pub format: TextureFormat,
}

/// One frame pushed through a [`TextureSink`].
///
/// The extent and format are plain public fields; the payload and the release
/// callback are private, so they stay together. Build a frame with
/// [`Self::new`] or [`Self::rgba8`] and attach a callback with
/// [`Self::on_release`]; read the payload in place with [`Self::payload`].
///
/// Dropping a frame that still holds its release callback runs it: a frame
/// nobody released — dropped by [`TextureSink::take_latest`]'s caller, or
/// lost to a panic unwinding past it — still tells its producer, exactly
/// once. Whoever ends up holding a frame either lets it go ([`Self::release`]
/// or a plain drop) or, in a bridge, splits it with the `unsafe`
/// [`Self::into_parts`]. Because the type implements `Drop`, it cannot be
/// destructured. A release callback must not panic: the guard around it only
/// applies to panic=unwind builds (see [`run_release`]).
pub struct TextureFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How the payload's pixels are laid out.
    pub format: TextureFormat,
    /// The pixels, or the platform buffer holding them.
    payload: FramePayload,
    /// Run exactly once when the frame is no longer needed — displaced in the
    /// mailbox, consumed, dropped with the last sink handle, or dropped while
    /// still holding it. `None` for a producer that needs no signal.
    release: Option<ReleaseCallback>,
}

/// The stand-in payload left behind by [`TextureFrame::into_parts`]. It is
/// never read — the frame is dropped right after — and shared so splitting a
/// frame allocates nothing.
static EMPTY_PIXELS: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from([]));

impl TextureFrame {
    /// A frame of `meta`'s extent and format over `payload`, with no release
    /// callback.
    #[must_use]
    pub fn new(meta: FrameMeta, payload: FramePayload) -> Self {
        Self {
            width: meta.width,
            height: meta.height,
            format: meta.format,
            payload,
            release: None,
        }
    }

    /// A CPU frame of tightly packed premultiplied RGBA8 with no release
    /// callback.
    #[must_use]
    pub fn rgba8(width: u32, height: u32, pixels: Arc<[u8]>) -> Self {
        Self::new(
            FrameMeta {
                width,
                height,
                format: TextureFormat::Rgba8,
            },
            FramePayload::Cpu(pixels),
        )
    }

    /// Attaches the callback run once the frame is no longer needed. A
    /// callback already attached runs first (through the same guard as every
    /// other release), so no callback is ever dropped unrun.
    #[must_use]
    pub fn on_release(mut self, release: impl FnOnce() + Send + 'static) -> Self {
        if let Some(displaced) = self.release.replace(Box::new(release)) {
            run_release(displaced);
        }
        self
    }

    /// The extent and format.
    #[must_use]
    pub fn meta(&self) -> FrameMeta {
        FrameMeta {
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }

    /// The pixels, or the platform buffer holding them, borrowed in place. The
    /// frame still owns the payload and its release callback.
    #[must_use]
    pub fn payload(&self) -> &FramePayload {
        &self.payload
    }

    /// Whether a release callback is attached.
    #[must_use]
    pub fn has_release(&self) -> bool {
        self.release.is_some()
    }

    /// Consumes the frame, handing over its extent and format, its payload and
    /// its release callback, and disarms the drop guard — the one way to move
    /// a native handle out of a frame. A caller that only needs to read the
    /// frame uses [`Self::meta`] and [`Self::payload`] instead.
    ///
    /// # Safety
    ///
    /// The caller must run the returned release callback exactly once, after
    /// the last use of the payload's native handle (or move it, with the
    /// handle, into a holder that does), and must not place that handle into
    /// another frame without it. The callback gives the producer's reference
    /// to the buffer back; a handle used after it ran points at a buffer the
    /// producer no longer keeps alive.
    #[must_use]
    pub unsafe fn into_parts(mut self) -> (FrameMeta, FramePayload, Option<ReleaseCallback>) {
        let meta = self.meta();
        let release = self.release.take();
        let payload = std::mem::replace(&mut self.payload, FramePayload::Cpu(EMPTY_PIXELS.clone()));
        (meta, payload, release)
    }

    /// Runs the release callback, if any, consuming the frame — what a holder
    /// that is done with a frame without passing it on calls. The same as
    /// dropping it, spelled out.
    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            run_release(release);
        }
    }
}

impl Drop for TextureFrame {
    /// A frame dropped still holding its callback has not been released by
    /// anyone, so it releases itself; a callback already run or taken on is
    /// `None` here.
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            run_release(release);
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

    /// A 1x1 frame whose release bumps the returned counter, then panics.
    fn panicking_frame() -> (TextureFrame, Arc<AtomicUsize>) {
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ran);
        let frame = TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            panic!("the release callback panics");
        });
        (frame, ran)
    }

    fn first_byte(frame: &TextureFrame) -> u8 {
        match frame.payload() {
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
        // SAFETY: the handle never reaches an importer — this case only
        // carries the address across the mailbox and compares it.
        let handle = unsafe { NativeHandle::new(NativeHandleKind::AHardwareBuffer, ptr) };
        sink.push(TextureFrame::new(
            FrameMeta {
                width: 4,
                height: 4,
                format: TextureFormat::Rgba8,
            },
            FramePayload::Native(handle),
        ));

        // Taken on another thread, as a render thread would.
        let frame = std::thread::spawn(move || sink.take_latest())
            .join()
            .unwrap()
            .expect("the native frame is pending");
        match frame.payload() {
            FramePayload::Native(handle) => {
                assert_eq!(handle.kind(), NativeHandleKind::AHardwareBuffer);
                assert_eq!(handle.ptr(), ptr, "the address is carried, never touched");
            }
            FramePayload::Cpu(_) => panic!("a native frame was pushed"),
        }
    }

    #[test]
    fn a_native_handle_round_trips_its_kind_and_pointer() {
        let mut byte = 0u8;
        let ptr: *mut c_void = (&raw mut byte).cast();
        for kind in [
            NativeHandleKind::CvPixelBuffer,
            NativeHandleKind::AHardwareBuffer,
        ] {
            // SAFETY: never handed to an importer; only the accessors read it.
            let handle = unsafe { NativeHandle::new(kind, ptr) };
            assert_eq!(handle.kind(), kind);
            assert_eq!(handle.ptr(), ptr);
        }
    }

    #[test]
    fn a_native_handle_is_move_only() {
        // Method resolution picks the `Clone` inherent-less fallback only when
        // the type is `Clone`: the blanket impl is the sole candidate for a
        // non-`Clone` type, so the two probes differ exactly then.
        trait Probe {
            fn is_clone(&self) -> bool {
                false
            }
        }
        impl<T> Probe for T {}
        struct Wrap<T>(std::marker::PhantomData<T>);
        impl<T: Clone> Wrap<T> {
            fn is_clone(&self) -> bool {
                true
            }
        }
        assert!(!Wrap::<NativeHandle>(std::marker::PhantomData).is_clone());
        assert!(!Wrap::<FramePayload>(std::marker::PhantomData).is_clone());
        assert!(Wrap::<Arc<[u8]>>(std::marker::PhantomData).is_clone());
    }

    #[test]
    fn into_parts_hands_over_the_callback_and_disarms_the_drop() {
        let released = Arc::new(AtomicUsize::new(0));
        // SAFETY: the callback is run below, once, and no native handle is
        // involved.
        let (meta, payload, release) = unsafe { counted(&released, 7).into_parts() };
        assert_eq!((meta.width, meta.height), (1, 1));
        assert!(matches!(&payload, FramePayload::Cpu(p) if p[0] == 7));
        assert_eq!(released.load(Ordering::SeqCst), 0, "consuming ran nothing");
        release.expect("the callback moved out")();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_frame_dropped_unreleased_releases_itself_once() {
        let released = Arc::new(AtomicUsize::new(0));
        drop(counted(&released, 0));
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "a dropped frame still tells its producer"
        );

        // A callback taken on by `into_parts` is the taker's, not the
        // frame's.
        let frame = counted(&released, 0);
        assert!(frame.has_release());
        // SAFETY: the callback is run below, once, and no native handle is
        // involved.
        let (_, _, taken) = unsafe { frame.into_parts() };
        assert_eq!(released.load(Ordering::SeqCst), 1, "splitting ran nothing");
        taken.expect("a callback is attached")();
        assert_eq!(released.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_taken_frame_dropped_by_its_taker_releases_once() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();
        sink.push(counted(&released, 1));
        let frame = sink.take_latest().expect("a frame is pending");
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(frame);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        drop(sink);
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "the empty mailbox releases nothing more"
        );
    }

    #[test]
    fn a_frame_lost_to_a_panic_releases_once() {
        let released = Arc::new(AtomicUsize::new(0));
        let frame = counted(&released, 2);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = frame;
            panic!("the holder panics before releasing");
        }));
        assert!(unwound.is_err());
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_replaced_mailbox_frame_releases_once_and_the_newest_on_teardown() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();
        sink.push(counted(&released, 1));
        sink.push(counted(&released, 2));
        sink.push(counted(&released, 3));
        assert_eq!(
            released.load(Ordering::SeqCst),
            2,
            "each replaced frame released as it was displaced"
        );
        drop(sink);
        assert_eq!(released.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_panicking_release_does_not_unwind_out_of_a_drop() {
        let (frame, ran) = panicking_frame();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(frame)));
        assert!(outcome.is_ok(), "the panic was caught inside the drop");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
    }

    #[test]
    fn a_panicking_explicit_release_does_not_propagate() {
        let (frame, ran) = panicking_frame();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            frame.release();
        }));
        assert!(outcome.is_ok());
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
    }

    #[test]
    fn a_panicking_displaced_release_leaves_the_mailbox_usable() {
        let released = Arc::new(AtomicUsize::new(0));
        let sink = TextureSink::new();
        let (displaced, ran) = panicking_frame();
        sink.push(displaced);

        // The displacing push runs the panicking callback on this thread.
        sink.push(counted(&released, 2));
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
        assert_eq!(released.load(Ordering::SeqCst), 0);
        let frame = sink.take_latest().expect("the newest frame is pending");
        assert_eq!(first_byte(&frame), 2, "the mailbox still accepts frames");
        frame.release();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_frame_debug_reports_whether_a_release_is_attached() {
        let frame = TextureFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(|| {});
        let text = format!("{frame:?}");
        assert!(text.contains("release: true"), "{text}");
    }
}

#[cfg(test)]
mod on_release_displacement {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn on_release_runs_the_callback_it_displaces_exactly_once() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let frame = TextureFrame::rgba8(1, 1, Arc::from([0u8; 4])).on_release({
            let first = Arc::clone(&first);
            move || {
                first.fetch_add(1, Ordering::SeqCst);
            }
        });
        let frame = frame.on_release({
            let second = Arc::clone(&second);
            move || {
                second.fetch_add(1, Ordering::SeqCst);
            }
        });
        assert_eq!(
            first.load(Ordering::SeqCst),
            1,
            "the displaced callback ran at once"
        );
        assert_eq!(second.load(Ordering::SeqCst), 0);
        drop(frame);
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(
            second.load(Ordering::SeqCst),
            1,
            "the attached callback ran on drop"
        );
    }
}
