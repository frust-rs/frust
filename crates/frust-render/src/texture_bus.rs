//! The **consumer** half of the external-texture frame bus: a frame *source*
//! — any closure that hands over the newest frame it has — registered here
//! becomes a scene texture the display list draws by id.
//!
//! # Shape
//!
//! [`register_texture_source`] takes a closure returning
//! `Option<`[`SourceFrame`]`>` and registers an
//! [`ExternalPass`](crate::ExternalPass) for it under an id of its own (the
//! reserved texture-source namespace, `SceneTextureId::for_texture_source`).
//! Once per engine frame, ahead of the scene, the pass polls the closure:
//!
//! - `None` — nothing new; whatever was shown last stays shown.
//! - a [`SourcePayload::Cpu`] frame — uploaded with `queue.write_texture` into
//!   an `Rgba8Unorm` texture the pass owns (re-created only when the extent
//!   or the device changes) and bound under the registration's id.
//! - a [`SourcePayload::Native`] frame — handed to the [`NativeImporter`]
//!   installed for its [`NativeHandleKind`] with [`set_native_importer`];
//!   with none installed it is refused, warned about once per registration
//!   and released.
//!
//! Every frame's [`SourceFrame::release`] callback runs exactly once, on every
//! path — including a panic unwinding through the pass (a [`SourceFrame`]
//! dropped still holding its callback runs it) and a device replaced by a
//! newer one (the bus polls the old device until its retire fences fired
//! before it lets go of it). A retire fence's callback holds its queue alive
//! until it fires, so a device that is never polled again keeps its retiring
//! frames' releases pending; a device lost outright releases them on its
//! next poll. *When* depends on what the bus did with the payload:
//!
//! - a CPU frame — right after the upload is enqueued (the queue copies the
//!   pixels into staging synchronously, so nothing reads the payload again);
//! - a refused frame (malformed, no importer, import error, or the retire
//!   backlog full) — right away, since nothing ever sampled it;
//! - an imported native frame — not when the importer returns. The release is
//!   the producer's signal that it may *reuse* the platform buffer (a camera
//!   or decoder ring recycles on it), and the GPU samples the imported
//!   texture until the frame is replaced and the submission that last drew it
//!   has finished. So the release is held until both hold: the imported
//!   texture was **retired** (a newer frame replaced it, or the registration
//!   was dropped / the device changed) and a `Queue::on_submitted_work_done`
//!   fence taken at that moment has fired (two chained fences for a dropped
//!   registration — what those cover, and what they do not, is under "A
//!   registration dropped mid-drain" below). It runs on whichever thread
//!   polls the device, and the imported texture's view is dropped just
//!   before it.
//!
//! An importer that needs the platform buffer alive for longer than its own
//! call keeps its own reference (in the imported texture's lifetime,
//! typically); the bus never releases that one.
//!
//! The returned [`TextureRegistration`] reports the id to draw by
//! ([`TextureRegistration::scene_texture_id`], what a widget's
//! `draw_scene_texture` names) and unregisters on drop.
//!
//! # Bounded retire backlog
//!
//! At most [`MAX_RETIRING`] (4) retired native textures per registration wait
//! on their fence at once, so a producer with a 3-deep buffer ring never sees
//! more than its ring in flight. When a native frame arrives with the backlog
//! full (after a non-blocking `Device::poll` gave the fences a chance), the
//! *newest* frame is dropped: its release runs at once, nothing was sampled,
//! and the last good frame stays shown.
//!
//! # A registration dropped mid-drain
//!
//! A registration dropped (or a device changed) while its imported texture is
//! still shown retires it through two chained fences: the second
//! `on_submitted_work_done` is taken when the first fires. What the two hops
//! guarantee is narrower than "the frame that sampled it finished": a fence
//! fires once every submission made *before it was taken* has completed, and
//! one taken with no work in flight fires at the next device poll. So the
//! release waits for every submission made before the drop, plus a second
//! completion point after that, and nothing more. A registration dropped
//! mid-drain — between a frame's record, which bound the view, and that
//! frame's submit — may release its last native frame while the frame that
//! sampled it is still executing (or not yet submitted): an interim submit
//! (atlas growth, another registration's backlog poll) can satisfy both hops
//! first. The producer may then overwrite its buffer and that one frame
//! tear. It is not a memory-safety hole — the importer holds its own
//! reference to the platform buffer for the imported texture's lifetime, and
//! `wgpu` keeps the texture alive for every submission that uses it — only a
//! possible visual glitch on the last frame of a dropped registration. Tying
//! the release to the dropping frame's own submission is follow-up work.
//!
//! # Why a closure, not a plugin type
//!
//! The renderer never depends on the plugin substrate, and the substrate never
//! names the renderer, so neither can hand the other its frame type. The
//! types here are this crate's own plain-data twins of the substrate's
//! producer-side ones, with matching constructors and accessors; the bridge is
//! a closure written by code that depends on both (a plugin's facade-side
//! half, or the app):
//!
//! ```ignore
//! let registration = register_texture_source(move || {
//!     let frame = sink.take_latest()?;
//!     // Check what can be refused while the frame still guards its release.
//!     if frame.meta().format != TextureFormat::Rgba8 {
//!         frame.release();
//!         return None;
//!     }
//!     // SAFETY: the callback is moved straight into the `SourceFrame` built
//!     // below, with the handle it protects, and nothing fallible runs in
//!     // between.
//!     let (meta, payload, release) = unsafe { frame.into_parts() };
//!     let payload = match payload {
//!         FramePayload::Cpu(pixels) => SourcePayload::Cpu(pixels),
//!         // SAFETY: forwards the producer's own `NativeHandle::new`
//!         // contract. The producer-side handle was consumed by value, so
//!         // this one is the only handle for the buffer, and it travels
//!         // with the release callback attached below.
//!         FramePayload::Native(h) => SourcePayload::Native(unsafe {
//!             NativeHandle::new(/* the matching NativeHandleKind */, h.ptr())
//!         }),
//!     };
//!     let mut out = SourceFrame::new(meta.width, meta.height, SourceFormat::Rgba8, payload);
//!     if let Some(release) = release {
//!         out = out.on_release(release);
//!     }
//!     Some(out)
//! });
//! ```
//!
//! Both frame types release on drop, so the bridge consumes the producer
//! frame by value and moves its payload and callback across together. The
//! handle is move-only, so it cannot be copied, and a frame's payload and
//! release are private, so outside `unsafe` a handle cannot leave the frame
//! that carries its release: the bridge's `into_parts` is the `unsafe` step
//! that promises the callback is carried on.
//!
//! # Pixels and compositing
//!
//! A CPU frame is tightly packed premultiplied-alpha RGBA8, `width * 4` bytes
//! per row, top row first, sampled as `Rgba8Unorm` (bytes pass through
//! unconverted). The engine composites the bound texture blended, never as
//! opaque — it cannot read the texels to know (`docs/LIMITATIONS.md`'s
//! `engine-scene-texture-always-blended`), and the bus inherits that.
//!
//! # Where it runs
//!
//! Exactly where external passes run: every engine-tier `SurfaceRenderer`
//! frame drains them, so a registration shows on the desktop, Android, iOS
//! and web shells alike, and nowhere a surface is not presenting. A shell that
//! rebuilds its engine on the same device (a fresh surface) gets the current
//! view bound again on its first frame; a *device* change drops what was
//! shown until the source yields its next frame.

use std::any::Any;
use std::ffi::c_void;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use frust_gpu::SceneTextureId;

use crate::external_pass::{
    ExternalFrame, ExternalPass, register_texture_source_pass, unregister_external_pass,
};

/// How many retired native textures one registration lets wait on their fence
/// at once (see the module docs).
pub const MAX_RETIRING: usize = 4;

/// The ordinal behind each registration's texture-source id.
static NEXT_SOURCE: AtomicU64 = AtomicU64::new(1);

/// The pixel layout of a [`SourceFrame`].
///
/// Non-exhaustive, mirroring the producer side: `Nv12` is the reserved next
/// variant, added once the bus can convert it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceFormat {
    /// 8-bit premultiplied RGBA, tightly packed (see the module docs).
    Rgba8,
}

/// Which platform object a [`NativeHandle`] points at — the key a
/// [`NativeImporter`] is installed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NativeHandleKind {
    /// An Apple `CVPixelBufferRef`.
    CvPixelBuffer,
    /// An Android `AHardwareBuffer*`.
    AHardwareBuffer,
}

/// An opaque platform GPU buffer a [`NativeImporter`] turns into a texture.
///
/// The bus never dereferences the pointer; only the importer installed for
/// its kind does. The frame's producer owns the buffer's lifetime and drops
/// its own reference in the frame's release callback.
///
/// The fields are private and the only constructor is the `unsafe`
/// [`Self::new`]: an importer dereferences the pointer on that constructor's
/// contract alone, so safe code cannot hand the bus a forged one.
///
/// A handle is move-only — neither `Clone` nor `Copy` — so it cannot be
/// copied, and a frame's payload and release callback are private to the
/// frame: outside `unsafe`, a handle cannot leave the frame that carries its
/// release (see [`SourceFrame::into_parts`]).
#[derive(Debug, PartialEq, Eq)]
pub struct NativeHandle {
    /// The platform object `ptr` points at.
    kind: NativeHandleKind,
    /// The platform object itself.
    ptr: *mut c_void,
}

impl NativeHandle {
    /// Wraps `ptr`, a platform object of `kind`, for a
    /// [`SourcePayload::Native`] frame.
    ///
    /// # Safety
    ///
    /// `ptr` is a live platform object of exactly `kind` — a
    /// `CVPixelBufferRef` for [`NativeHandleKind::CvPixelBuffer`], an
    /// `AHardwareBuffer*` for [`NativeHandleKind::AHardwareBuffer`] — on which
    /// the producer holds a retained reference of its own (`CFRetain`,
    /// `AHardwareBuffer_acquire`), kept until the release callback of the one
    /// frame this handle is placed in has run (the handle is move-only, so it
    /// cannot be copied, and outside `unsafe` it cannot leave that frame
    /// without the callback). The [`NativeImporter`] installed
    /// for `kind` dereferences the pointer (describes, imports and retains the
    /// object) on this promise alone, so a dangling, foreign or mistyped
    /// pointer is undefined behaviour there. A bridge from the plugin
    /// substrate's own handle forwards that handle's identical contract.
    #[must_use]
    pub unsafe fn new(kind: NativeHandleKind, ptr: *mut c_void) -> Self {
        Self { kind, ptr }
    }

    /// The platform object [`Self::ptr`] points at.
    #[must_use]
    pub fn kind(&self) -> NativeHandleKind {
        self.kind
    }

    /// The platform object itself, as handed to [`Self::new`].
    #[must_use]
    pub fn ptr(&self) -> *mut c_void {
        self.ptr
    }
}

// SAFETY: an inert address plus a tag — moving it between threads moves only
// the address, and nothing here dereferences it. Both kinds it can name are
// reference-counted platform objects that are safe to retain, release and
// import from any thread, and the frame carrying the handle also carries the
// producer's own reference (released by its callback), so the object outlives
// the address's trip to the render thread.
unsafe impl Send for NativeHandle {}

/// Where a [`SourceFrame`]'s pixels live.
///
/// Not `Clone`: a native payload holds a move-only [`NativeHandle`].
#[derive(Debug, PartialEq, Eq)]
pub enum SourcePayload {
    /// Pixels in CPU memory, laid out per the frame's [`SourceFormat`].
    Cpu(Arc<[u8]>),
    /// A platform GPU buffer, imported rather than uploaded.
    Native(NativeHandle),
}

/// The callback a frame carries to tell its producer the bus is done with it.
pub type ReleaseCallback = Box<dyn FnOnce() + Send>;

/// The extent and pixel format of a [`SourceFrame`], as returned by
/// [`SourceFrame::into_parts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMeta {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How the payload's pixels are laid out.
    pub format: SourceFormat,
}

/// One frame a source hands the bus.
///
/// The extent and format are plain public fields; the payload and the release
/// callback are private, so they stay together. Build a frame with
/// [`Self::new`] or [`Self::rgba8`] and attach a callback with
/// [`Self::on_release`]; read the payload in place with [`Self::payload`].
///
/// Dropping a frame that still holds its release callback runs it: the bus
/// runs it for every frame it is handed (see the module docs), and a frame
/// nobody released — dropped by a holder that never passed it on, or lost to
/// a panic in the source or the pass — still releases, exactly once. Because
/// the type implements `Drop`, it cannot be destructured; the bus splits it
/// with the `unsafe` [`Self::into_parts`]. A release callback must not panic:
/// the guard around it only applies to panic=unwind builds (see
/// [`run_release`]).
pub struct SourceFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How the payload's pixels are laid out.
    pub format: SourceFormat,
    /// The pixels, or the platform buffer holding them.
    payload: SourcePayload,
    /// Run exactly once when the bus is done with the frame: right after the
    /// upload for a CPU frame, at once for a refused one, and for an imported
    /// native frame only after its texture was replaced or unbound and the GPU
    /// work that sampled it finished — the producer may reuse the platform
    /// buffer from this point on (see the module docs). A frame dropped while
    /// still holding it runs it then.
    release: Option<ReleaseCallback>,
}

/// The stand-in payload left behind by [`SourceFrame::into_parts`]. It is
/// never read — the frame is dropped right after — and shared so splitting a
/// frame allocates nothing.
static EMPTY_PIXELS: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from([]));

impl SourceFrame {
    /// A frame of the given extent and format over `payload`, with no release
    /// callback.
    #[must_use]
    pub fn new(width: u32, height: u32, format: SourceFormat, payload: SourcePayload) -> Self {
        Self {
            width,
            height,
            format,
            payload,
            release: None,
        }
    }

    /// A CPU frame of tightly packed premultiplied RGBA8 with no release
    /// callback.
    #[must_use]
    pub fn rgba8(width: u32, height: u32, pixels: Arc<[u8]>) -> Self {
        Self::new(
            width,
            height,
            SourceFormat::Rgba8,
            SourcePayload::Cpu(pixels),
        )
    }

    /// Attaches the callback run once the bus is done with the frame,
    /// replacing any callback already attached (which is dropped unrun).
    #[must_use]
    pub fn on_release(mut self, release: impl FnOnce() + Send + 'static) -> Self {
        self.release = Some(Box::new(release));
        self
    }

    /// The extent and format.
    #[must_use]
    pub fn meta(&self) -> SourceMeta {
        SourceMeta {
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }

    /// The pixels, or the platform buffer holding them, borrowed in place. The
    /// frame still owns the payload and its release callback.
    #[must_use]
    pub fn payload(&self) -> &SourcePayload {
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
    pub unsafe fn into_parts(mut self) -> (SourceMeta, SourcePayload, Option<ReleaseCallback>) {
        let meta = self.meta();
        let release = self.release.take();
        let payload =
            std::mem::replace(&mut self.payload, SourcePayload::Cpu(EMPTY_PIXELS.clone()));
        (meta, payload, release)
    }

    /// Runs the release callback, if any, consuming the frame — the same as
    /// dropping it, spelled out.
    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            run_release(release);
        }
    }
}

impl Drop for SourceFrame {
    /// A frame dropped still holding its callback was released by no one, so
    /// it releases itself; one whose callback already ran or was taken on
    /// (into a held import) is `None` here.
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            run_release(release);
        }
    }
}

impl fmt::Debug for SourceFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("payload", &self.payload)
            .field("release", &self.release.is_some())
            .finish()
    }
}

/// What a [`NativeImporter`] produced: a view the engine can sample, and its
/// extent in texels.
///
/// `view` must be a non-array 2D view of a float-sampleable texture with
/// `TEXTURE_BINDING` usage, created on the device the importer was handed —
/// the same requirements as `ExternalFrame::bind_texture`.
#[derive(Debug)]
pub struct ImportedTexture {
    /// The view bound under the registration's id.
    pub view: wgpu::TextureView,
    /// The view's extent in texels.
    pub size: (u32, u32),
}

/// Why a [`NativeImporter`] could not import a handle. The frame is released
/// and the last good frame stays shown.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportError {
    /// The device or backend cannot import this kind of buffer at all (the
    /// wrong graphics API, a missing extension).
    Unsupported(String),
    /// This particular buffer could not be imported (an unsupported pixel
    /// format, a driver error).
    Failed(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "native import unsupported: {reason}"),
            Self::Failed(reason) => write!(f, "native import failed: {reason}"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Turns a platform GPU buffer into a texture on the frame's device, without
/// a CPU copy.
///
/// Installed per [`NativeHandleKind`] with [`set_native_importer`], and called
/// on the render thread inside the frame's external-pass drain — so it records
/// nothing of its own and never submits; it creates (or looks up) a texture
/// over the buffer and returns a view of it. `Send + Sync` because the table
/// it is installed in is process-wide.
pub trait NativeImporter: Send + Sync {
    /// Imports `handle` on `device`. The bus holds the frame's release
    /// callback until the imported texture is retired and the GPU finished
    /// with it, but the platform buffer is still the importer's to keep alive:
    /// an importer whose texture aliases it keeps its own reference for the
    /// texture's lifetime.
    ///
    /// # Errors
    ///
    /// [`ImportError`] when the handle cannot be imported on this device.
    fn import(
        &self,
        handle: &NativeHandle,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<ImportedTexture, ImportError>;
}

/// The installed importers, one per kind at most.
static IMPORTERS: Mutex<Vec<(NativeHandleKind, Arc<dyn NativeImporter>)>> = Mutex::new(Vec::new());

fn importers() -> MutexGuard<'static, Vec<(NativeHandleKind, Arc<dyn NativeImporter>)>> {
    IMPORTERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Installs `importer` for every [`SourcePayload::Native`] frame of `kind`,
/// process-wide, returning the importer it replaced.
///
/// Takes effect from the next frame a registration is handed; frames of a
/// kind with no importer are refused (see the module docs).
pub fn set_native_importer(
    kind: NativeHandleKind,
    importer: Arc<dyn NativeImporter>,
) -> Option<Arc<dyn NativeImporter>> {
    let mut importers = importers();
    match importers.iter_mut().find(|(k, _)| *k == kind) {
        Some((_, slot)) => Some(std::mem::replace(slot, importer)),
        None => {
            importers.push((kind, importer));
            None
        }
    }
}

/// Removes the importer installed for `kind`, returning it.
pub fn clear_native_importer(kind: NativeHandleKind) -> Option<Arc<dyn NativeImporter>> {
    let mut importers = importers();
    let index = importers.iter().position(|(k, _)| *k == kind)?;
    Some(importers.remove(index).1)
}

/// Whether an importer is installed for `kind` right now.
///
/// What a producer asks before choosing a payload: a [`SourcePayload::Native`]
/// frame of a kind with no importer is refused, so a producer that can also
/// copy into CPU memory sends [`SourcePayload::Cpu`] frames instead. The
/// answer can change when an importer is installed or cleared later, so a
/// producer asks once per session it opens rather than once per process.
#[must_use]
pub fn native_importer_installed(kind: NativeHandleKind) -> bool {
    native_importer(kind).is_some()
}

fn native_importer(kind: NativeHandleKind) -> Option<Arc<dyn NativeImporter>> {
    importers()
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, importer)| Arc::clone(importer))
}

/// A frame source registered with [`register_texture_source`]; unregisters on
/// drop.
#[must_use = "dropping the registration unregisters the frame source at once"]
pub struct TextureRegistration {
    id: SceneTextureId,
    status: Arc<Status>,
}

impl TextureRegistration {
    /// The id a display list draws this source by —
    /// `PaintScene::draw_scene_texture`'s / `texture_view`'s argument.
    #[must_use]
    pub fn scene_texture_id(&self) -> u64 {
        self.id.get()
    }

    /// The extent of the frame currently shown, `None` before the first one.
    ///
    /// Read by an app that sizes or fits its view to the content (a video's
    /// aspect ratio); updated on the render thread as frames land.
    #[must_use]
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        unpack(self.status.size.load(Ordering::Acquire))
    }
}

impl Drop for TextureRegistration {
    fn drop(&mut self) {
        unregister_external_pass(self.id);
    }
}

impl fmt::Debug for TextureRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextureRegistration")
            .field("scene_texture_id", &self.id.get())
            .field("frame_size", &self.frame_size())
            .finish()
    }
}

/// Registers `source` as a scene texture, polled once per engine frame for
/// its newest frame (see the module docs), and returns the registration that
/// names it and keeps it alive.
///
/// `source` runs on the render thread with the frame's external-pass drain in
/// progress: it should only hand over a frame it already has (take from a
/// mailbox), never block for one. A source — or an importer — that panics is
/// retired like any panicking external pass, and the frame in flight is not
/// leaked: a [`SourceFrame`] (like the plugin substrate's twin) dropped while
/// still holding its release callback runs it during the unwind.
pub fn register_texture_source(
    source: impl FnMut() -> Option<SourceFrame> + Send + 'static,
) -> TextureRegistration {
    let id = SceneTextureId::for_texture_source(NEXT_SOURCE.fetch_add(1, Ordering::Relaxed));
    let status = Arc::new(Status::default());
    let pass = SourcePass {
        state: Mutex::new(PassState {
            source: Box::new(source),
            shown: None,
            retiring: Arc::new(AtomicUsize::new(0)),
            warned: Warned::default(),
        }),
        status: Arc::clone(&status),
    };
    let registered = register_texture_source_pass(id, Arc::new(pass));
    debug_assert!(
        registered,
        "a fresh texture-source id is never already claimed"
    );
    TextureRegistration { id, status }
}

/// What a registration reports back across threads.
#[derive(Default)]
struct Status {
    /// The shown frame's extent, packed by [`pack`]; `0` before the first.
    size: AtomicU64,
}

fn pack((width, height): (u32, u32)) -> u64 {
    (u64::from(width) << 32) | u64::from(height)
}

fn unpack(packed: u64) -> Option<(u32, u32)> {
    // Both halves are non-zero for every extent the bus shows, so `0` is free
    // to mean "nothing yet".
    (packed != 0).then_some(((packed >> 32) as u32, packed as u32))
}

/// The boxed frame source a pass polls.
type FrameSource = Box<dyn FnMut() -> Option<SourceFrame> + Send>;

/// One registration's pass.
struct SourcePass {
    /// Serializes `record`: a pass may be called concurrently by two
    /// engine-tier surfaces, and the source is `FnMut`.
    state: Mutex<PassState>,
    status: Arc<Status>,
}

struct PassState {
    source: FrameSource,
    /// What is shown now, kept so it can be bound again into a rebuilt engine.
    shown: Option<Shown>,
    /// Retired native textures still waiting on their fence.
    retiring: Arc<AtomicUsize>,
    warned: Warned,
}

/// The view a registration currently shows.
struct Shown {
    /// The device `view` was created on — compared against each frame's, as
    /// the device-change check. Held only alongside the resources made on it
    /// and replaced with them.
    device: wgpu::Device,
    view: wgpu::TextureView,
    size: (u32, u32),
    /// The bus's own upload texture behind `view`, reused by the next CPU
    /// frame of the same extent; `None` for an imported view.
    upload: Option<wgpu::Texture>,
    /// The imported frame's release, held until the view is retired and
    /// fenced; `None` for a bus upload.
    native: Option<NativeHold>,
}

/// Completion fence: runs the callback once the work submitted so far finished.
type Fence = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

fn queue_fence(queue: &wgpu::Queue) -> Fence {
    let queue = queue.clone();
    Arc::new(move |callback| queue.on_submitted_work_done(callback))
}

/// A retired native texture on its way out: dropping it (when its fence fires,
/// or unrun because the device is gone) frees the view and runs the release.
struct Retiring {
    keep: Option<Box<dyn Any + Send>>,
    release: Option<ReleaseCallback>,
    pending: Arc<AtomicUsize>,
}

impl Drop for Retiring {
    fn drop(&mut self) {
        drop(self.keep.take());
        self.pending.fetch_sub(1, Ordering::AcqRel);
        if let Some(release) = self.release.take() {
            run_release(release);
        }
    }
}

/// Runs a frame's release callback behind `catch_unwind`, the last line of
/// defence for a callback that panics anyway. Releases run from `Drop` impls
/// (a retired texture's fence, a frame dropped unrun), sometimes during an
/// unwind, and a panic escaping one there aborts the process. The guard only
/// works in panic=unwind builds (dev and test): the shipped release profile
/// sets `panic = "abort"`, where a panicking callback aborts the process and
/// the must-not-panic rule is the only protection. A caught panic is logged;
/// a callback must still not panic (see [`SourceFrame`]).
fn run_release(release: ReleaseCallback) {
    if std::panic::catch_unwind(AssertUnwindSafe(release)).is_err() {
        log::error!("texture source: a frame release callback panicked and was caught");
    }
}

/// Starts retiring `keep` and `release` behind `hops` successive fences; the
/// second and later fences are taken from inside the previous one's callback.
fn retire(
    keep: Box<dyn Any + Send>,
    release: Option<ReleaseCallback>,
    pending: &Arc<AtomicUsize>,
    fence: &Fence,
    hops: u32,
) {
    pending.fetch_add(1, Ordering::AcqRel);
    let token = Retiring {
        keep: Some(keep),
        release,
        pending: Arc::clone(pending),
    };
    defer(token, Arc::clone(fence), hops);
}

fn defer(token: Retiring, fence: Fence, hops: u32) {
    let next = Arc::clone(&fence);
    fence(Box::new(move || {
        if hops > 1 {
            defer(token, next, hops - 1);
        } else {
            drop(token);
        }
    }));
}

/// What an imported view keeps until it is retired.
struct NativeHold {
    view: Option<wgpu::TextureView>,
    release: Option<ReleaseCallback>,
    queue: wgpu::Queue,
    pending: Arc<AtomicUsize>,
}

impl NativeHold {
    /// Retires on replacement: every submission that sampled the view
    /// happened before now, so one fence covers them.
    fn retire_replaced(mut self) {
        self.retire_with(1);
    }

    fn retire_with(&mut self, hops: u32) {
        if let Some(view) = self.view.take() {
            retire(
                Box::new(view),
                self.release.take(),
                &self.pending,
                &queue_fence(&self.queue),
                hops,
            );
        }
    }
}

impl Drop for NativeHold {
    /// Unbind (registration dropped, device change): chains two fences. They
    /// cover every submission made before the drop, not a frame recorded
    /// before it and submitted after — a drop mid-drain may release the
    /// producer's buffer while that last frame still samples it (the module
    /// docs' "A registration dropped mid-drain").
    fn drop(&mut self) {
        self.retire_with(2);
    }
}

/// Installs `new` as the shown view, retiring a native one it replaces.
fn replace_shown(state: &mut PassState, new: Shown) {
    if let Some(mut old) = state.shown.replace(new)
        && let Some(hold) = old.native.take()
    {
        hold.retire_replaced();
    }
}

/// Which refusals a registration has already warned about; later ones of the
/// same kind are logged at debug level.
#[derive(Default)]
struct Warned {
    malformed: bool,
    missing_importer: bool,
    import_failed: bool,
    backlog: bool,
}

/// Logs a refusal at `warn!` the first time `flag` sees it, `debug!` after.
fn report(flag: &mut bool, id: u64, message: fmt::Arguments<'_>) {
    if *flag {
        log::debug!("texture source {id}: {message}");
    } else {
        *flag = true;
        log::warn!(
            "texture source {id}: {message} (further refusals of this kind are logged at \
             debug level)"
        );
    }
}

impl ExternalPass for SourcePass {
    fn record(&self, frame: &mut ExternalFrame<'_>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *state;

        // A view made on another device is useless here: drop it, and with it
        // the upload texture, rather than bind across devices.
        if state
            .shown
            .as_ref()
            .is_some_and(|shown| shown.device != *frame.device())
        {
            let old_device = state.shown.as_ref().map(|shown| shown.device.clone());
            state.shown = None;
            // Dropping the view retired it on the OLD queue, whose fences fire
            // only when that device is polled; nothing else will poll it again,
            // so drain both hops here before the device is let go of.
            if let Some(old_device) = old_device {
                for _ in 0..2 {
                    let _ = old_device.poll(wgpu::PollType::wait_indefinitely());
                }
            }
        }

        // The frame keeps its release callback until the bus is done with
        // it, so a panic anywhere below still releases it (on drop).
        let mut rebind = false;
        if let Some(source_frame) = (state.source)() {
            // SAFETY: the callback goes straight into a `ReleaseGuard`, which
            // runs it on every path out (drop included) or hands it to an
            // import that owes the call; the handle never leaves this scope
            // apart from that import.
            let (meta, payload, release) = unsafe { source_frame.into_parts() };
            // Until the release callback is handed on, a panic below drops it
            // with this frame's locals and so still runs it.
            let release = ReleaseGuard(release);
            rebind = match payload {
                SourcePayload::Cpu(pixels) => {
                    let size = (meta.width, meta.height);
                    let changed = upload(state, frame, size, meta.format, &pixels);
                    // The queue copied the pixels into staging: nothing reads
                    // the payload again.
                    release.release();
                    changed
                }
                SourcePayload::Native(handle) => import(state, frame, handle, release),
            };
        }

        if let Some(shown) = &state.shown {
            // A changed view is bound; so is an unchanged one the engine no
            // longer holds (a shell rebuilt it for a fresh surface).
            if rebind || !frame.has_binding() {
                frame.bind_texture(shown.size, shown.view.clone());
            }
            self.status.size.store(pack(shown.size), Ordering::Release);
        }
    }
}

/// Uploads one CPU frame, answering whether the shown view changed (and so
/// must be bound). A malformed frame is refused and changes nothing.
fn upload(
    state: &mut PassState,
    frame: &mut ExternalFrame<'_>,
    size: (u32, u32),
    format: SourceFormat,
    pixels: &[u8],
) -> bool {
    let id = frame.id().get();
    let (width, height) = size;
    let SourceFormat::Rgba8 = format;
    let max = frame.device().limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        report(
            &mut state.warned.malformed,
            id,
            format_args!("refused a {width}x{height} frame: the extent is empty or past {max}"),
        );
        return false;
    }
    let expected = u64::from(width) * u64::from(height) * 4;
    if pixels.len() as u64 != expected {
        report(
            &mut state.warned.malformed,
            id,
            format_args!(
                "refused a {width}x{height} RGBA8 frame of {} bytes: tightly packed rows need \
                 {expected}",
                pixels.len()
            ),
        );
        return false;
    }

    let reusable = state
        .shown
        .as_ref()
        .and_then(|shown| shown.upload.as_ref().filter(|_| shown.size == size));
    let (texture, changed) = match reusable {
        Some(texture) => (texture.clone(), false),
        None => {
            let texture = frame.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("frust texture-bus frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            (texture, true)
        }
    };

    // Staged on the queue, so it lands ahead of the frame's own command
    // buffer when the frame submits.
    frame.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    if changed {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        replace_shown(
            state,
            Shown {
                device: frame.device().clone(),
                view,
                size,
                upload: Some(texture),
                native: None,
            },
        );
    }
    changed
}

/// A frame's release callback, held between taking a frame apart and the
/// point where the bus either runs it or moves it into an import. Dropped
/// still holding the callback (a panic in between) it runs it, so the
/// exactly-once promise of [`SourceFrame`] survives consuming the frame.
struct ReleaseGuard(Option<ReleaseCallback>);

impl ReleaseGuard {
    /// Runs the callback now.
    fn release(mut self) {
        if let Some(release) = self.0.take() {
            run_release(release);
        }
    }

    /// Hands the callback on to a new holder, which now owes the call.
    fn take(mut self) -> Option<ReleaseCallback> {
        self.0.take()
    }
}

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            run_release(release);
        }
    }
}

/// Refuses a frame: nothing sampled it, so it is released at once. Answers
/// "the shown view did not change".
fn refuse(release: ReleaseGuard) -> bool {
    release.release();
    false
}

/// Whether a native frame finds the retire backlog full: `pending` is at
/// [`MAX_RETIRING`] even after `poll` — a non-blocking device poll, run only
/// when the backlog looks full — gave completed fences their chance to run.
fn backlog_full(pending: &AtomicUsize, poll: impl FnOnce()) -> bool {
    if pending.load(Ordering::Acquire) < MAX_RETIRING {
        return false;
    }
    poll();
    pending.load(Ordering::Acquire) >= MAX_RETIRING
}

/// Whether the retire backlog refuses a native frame of `kind` now (see
/// [`backlog_full`]), reporting the refusal — at `warn!` once per
/// registration. Touches nothing else: the shown view stays as it is.
fn backlog_refuses(
    state: &mut PassState,
    id: u64,
    kind: NativeHandleKind,
    poll: impl FnOnce(),
) -> bool {
    if !backlog_full(&state.retiring, poll) {
        return false;
    }
    report(
        &mut state.warned.backlog,
        id,
        format_args!(
            "refused a {kind:?} frame: {MAX_RETIRING} retired frames still wait on the GPU"
        ),
    );
    true
}

/// Imports one native frame through the importer installed for its kind,
/// answering whether the shown view changed. Refused (and warned about once)
/// when no importer is installed, the retire backlog is full or the import
/// fails; the last good frame stays shown and the refused frame is released
/// at once. An imported frame's `release` is held until its view is retired
/// and fenced (see the module docs).
fn import(
    state: &mut PassState,
    frame: &mut ExternalFrame<'_>,
    handle: NativeHandle,
    release: ReleaseGuard,
) -> bool {
    let id = frame.id().get();
    let Some(importer) = native_importer(handle.kind()) else {
        report(
            &mut state.warned.missing_importer,
            id,
            format_args!(
                "refused a {:?} frame: no native importer is installed for that kind",
                handle.kind()
            ),
        );
        return refuse(release);
    };
    if backlog_refuses(state, id, handle.kind(), || {
        let _ = frame.device().poll(wgpu::PollType::Poll);
    }) {
        return refuse(release);
    }
    match importer.import(&handle, frame.device(), frame.queue()) {
        Ok(imported) => {
            let hold = NativeHold {
                view: Some(imported.view.clone()),
                release: release.take(),
                queue: frame.queue().clone(),
                pending: Arc::clone(&state.retiring),
            };
            replace_shown(
                state,
                Shown {
                    device: frame.device().clone(),
                    view: imported.view,
                    size: imported.size,
                    upload: None,
                    native: Some(hold),
                },
            );
            true
        }
        Err(error) => {
            report(
                &mut state.warned.import_failed,
                id,
                format_args!("refused a {:?} frame: {error}", handle.kind()),
            );
            refuse(release)
        }
    }
}

// Registration itself touches the process-wide external-pass registry, whose
// own unit tests assert on it; it is exercised from `tests/texture_bus.rs`, a
// binary of its own, instead.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packed_extent_round_trips_and_zero_means_none() {
        assert_eq!(unpack(0), None);
        assert_eq!(unpack(pack((64, 32))), Some((64, 32)));
        assert_eq!(unpack(pack((u32::MAX, 1))), Some((u32::MAX, 1)));
    }

    #[test]
    fn a_source_frame_releases_once_and_reports_its_callback_in_debug() {
        let released = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&released);
        let frame = SourceFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(format!("{frame:?}").contains("release: true"));
        frame.release();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn import_errors_say_which_kind_of_failure() {
        assert!(
            ImportError::Unsupported("no extension".into())
                .to_string()
                .contains("unsupported")
        );
        assert!(
            ImportError::Failed("bad format".into())
                .to_string()
                .contains("bad format")
        );
    }

    struct NoImport;

    impl NativeImporter for NoImport {
        fn import(
            &self,
            _handle: &NativeHandle,
            _device: &wgpu::Device,
            _queue: &wgpu::Queue,
        ) -> Result<ImportedTexture, ImportError> {
            Err(ImportError::Unsupported("test importer".into()))
        }
    }

    #[test]
    fn the_installed_query_follows_set_and_clear_per_kind() {
        // The only unit test in this binary that touches the process-wide
        // table, and it uses the Apple kind the host never installs.
        let kind = NativeHandleKind::CvPixelBuffer;
        assert!(!native_importer_installed(kind));
        assert!(set_native_importer(kind, Arc::new(NoImport)).is_none());
        assert!(native_importer_installed(kind));
        assert!(!native_importer_installed(
            NativeHandleKind::AHardwareBuffer
        ));
        assert!(clear_native_importer(kind).is_some());
        assert!(!native_importer_installed(kind));
    }

    /// A fence that parks its callbacks until the test fires them.
    type Parked = Vec<Box<dyn FnOnce() + Send>>;

    #[derive(Clone, Default)]
    struct FakeFence(Arc<Mutex<Parked>>);

    impl FakeFence {
        fn fence(&self) -> Fence {
            let parked = Arc::clone(&self.0);
            Arc::new(move |callback| parked.lock().unwrap().push(callback))
        }

        /// Fires every callback parked so far (not ones they park in turn).
        fn fire(&self) {
            let batch = std::mem::take(&mut *self.0.lock().unwrap());
            for callback in batch {
                callback();
            }
        }
    }

    fn retire_counting(
        fake: &FakeFence,
        pending: &Arc<AtomicUsize>,
        released: &Arc<AtomicU64>,
        hops: u32,
    ) {
        let released = Arc::clone(released);
        retire(
            Box::new(()),
            Some(Box::new(move || {
                released.fetch_add(1, Ordering::SeqCst);
            })),
            pending,
            &fake.fence(),
            hops,
        );
    }

    #[test]
    fn a_retired_release_waits_for_its_fence_and_runs_once() {
        let fake = FakeFence::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicU64::new(0));
        retire_counting(&fake, &pending, &released, 1);
        assert_eq!(pending.load(Ordering::SeqCst), 1);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        fake.fire();
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(pending.load(Ordering::SeqCst), 0);
        fake.fire();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_chained_retire_needs_a_fence_per_hop() {
        let fake = FakeFence::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicU64::new(0));
        retire_counting(&fake, &pending, &released, 2);
        fake.fire();
        assert_eq!(
            released.load(Ordering::SeqCst),
            0,
            "one fence is not enough"
        );
        assert_eq!(pending.load(Ordering::SeqCst), 1);
        fake.fire();
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(pending.load(Ordering::SeqCst), 0);
    }

    /// A frame source standing in for a registration's, never polled here.
    fn idle_state() -> PassState {
        PassState {
            source: Box::new(|| None),
            shown: None,
            retiring: Arc::new(AtomicUsize::new(0)),
            warned: Warned::default(),
        }
    }

    /// A release callback that bumps the returned counter, then panics.
    fn panicking_release() -> (ReleaseCallback, Arc<AtomicU64>) {
        let ran = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&ran);
        let callback: ReleaseCallback = Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            panic!("the release callback panics");
        });
        (callback, ran)
    }

    fn counted_frame(released: &Arc<AtomicU64>) -> SourceFrame {
        let released = Arc::clone(released);
        SourceFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(move || {
            released.fetch_add(1, Ordering::SeqCst);
        })
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
    fn a_native_handle_and_payload_are_move_only() {
        // The blanket-trait probe: the `Clone`-bounded inherent method wins
        // method resolution only for a type that is `Clone`.
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
        assert!(!Wrap::<SourcePayload>(std::marker::PhantomData).is_clone());
        assert!(Wrap::<Arc<[u8]>>(std::marker::PhantomData).is_clone());
    }

    #[test]
    fn into_parts_hands_over_the_callback_and_disarms_the_drop() {
        let released = Arc::new(AtomicU64::new(0));
        // SAFETY: the callback is run below, once, and no native handle is
        // involved.
        let (meta, payload, release) = unsafe { counted_frame(&released).into_parts() };
        assert_eq!((meta.width, meta.height), (1, 1));
        assert!(matches!(&payload, SourcePayload::Cpu(p) if p.len() == 4));
        assert_eq!(released.load(Ordering::SeqCst), 0, "consuming ran nothing");
        release.expect("the callback moved out")();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_source_frame_dropped_unconsumed_releases_once() {
        let released = Arc::new(AtomicU64::new(0));
        drop(counted_frame(&released));
        assert_eq!(released.load(Ordering::SeqCst), 1);

        // A callback taken on (as an import holds it) is not run by the drop.
        let frame = counted_frame(&released);
        assert!(frame.has_release());
        // SAFETY: the callback is run below, once, and no native handle is
        // involved.
        let (_, _, held) = unsafe { frame.into_parts() };
        assert_eq!(released.load(Ordering::SeqCst), 1);
        held.expect("a callback is attached")();
        assert_eq!(released.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_panicking_source_frame_release_does_not_unwind_out_of_a_drop() {
        let (callback, ran) = panicking_release();
        let frame = SourceFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(callback);
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || drop(frame)));
        assert!(outcome.is_ok(), "the panic was caught inside the drop");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
    }

    #[test]
    fn a_panicking_explicit_source_frame_release_does_not_propagate() {
        let (callback, ran) = panicking_release();
        let frame = SourceFrame::rgba8(1, 1, Arc::from(vec![0u8; 4])).on_release(callback);
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || frame.release()));
        assert!(outcome.is_ok());
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
    }

    #[test]
    fn a_panicking_retired_release_does_not_propagate() {
        let (callback, ran) = panicking_release();
        let pending = Arc::new(AtomicUsize::new(1));
        let retiring = Retiring {
            keep: None,
            release: Some(callback),
            pending: Arc::clone(&pending),
        };
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || drop(retiring)));
        assert!(outcome.is_ok());
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
        assert_eq!(pending.load(Ordering::SeqCst), 0, "the backlog slot freed");
    }

    #[test]
    fn a_panicking_bus_guard_release_does_not_propagate() {
        let (callback, ran) = panicking_release();
        let guard = ReleaseGuard(Some(callback));
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || guard.release()));
        assert!(outcome.is_ok());
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");

        // A guard dropped unreleased (a panic between taking a frame apart
        // and handing its callback on) is the same path through `Drop`.
        let (callback, ran) = panicking_release();
        let guard = ReleaseGuard(Some(callback));
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || drop(guard)));
        assert!(outcome.is_ok());
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the callback ran once");
    }

    #[test]
    fn a_source_frame_lost_to_a_panicking_pass_releases_once() {
        let released = Arc::new(AtomicU64::new(0));
        let mut source: FrameSource = {
            let released = Arc::clone(&released);
            Box::new(move || Some(counted_frame(&released)))
        };
        // The pass's shape: take a frame from the source, then fail before
        // the bus is done with it.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _frame = source().expect("the source hands a frame over");
            panic!("the pass panics while it holds the frame");
        }));
        assert!(unwound.is_err());
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_retire_backlog_tolerates_four_and_refuses_the_fifth_once_warned() {
        let fake = FakeFence::default();
        let mut state = idle_state();
        let retired = Arc::new(AtomicU64::new(0));
        let polls = AtomicUsize::new(0);
        let poll = || {
            polls.fetch_add(1, Ordering::SeqCst);
        };

        // Four frames are admitted, each retired (still fenced) by the next.
        for _ in 0..MAX_RETIRING {
            assert!(!backlog_refuses(
                &mut state,
                7,
                NativeHandleKind::AHardwareBuffer,
                poll
            ));
            retire_counting(&fake, &state.retiring, &retired, 1);
        }
        assert_eq!(state.retiring.load(Ordering::SeqCst), MAX_RETIRING);
        assert_eq!(polls.load(Ordering::SeqCst), 0, "no poll below the bound");
        assert!(!state.warned.backlog);

        // The fifth is refused after one poll, and released at once.
        let released = Arc::new(AtomicU64::new(0));
        let fifth = counted_frame(&released);
        assert!(backlog_refuses(
            &mut state,
            7,
            NativeHandleKind::AHardwareBuffer,
            poll
        ));
        assert!(
            // SAFETY: the guard runs the callback inside `refuse`.
            !refuse(ReleaseGuard(unsafe { fifth.into_parts() }.2)),
            "a refusal never changes the shown view"
        );
        assert_eq!(released.load(Ordering::SeqCst), 1, "released at once");
        assert_eq!(polls.load(Ordering::SeqCst), 1, "fences got one poll");
        assert!(state.warned.backlog, "the first refusal is the warning");
        assert!(state.shown.is_none(), "the shown view is untouched");
        assert_eq!(retired.load(Ordering::SeqCst), 0);

        // A sixth is refused too; the warning flag was already spent, so it
        // is logged at debug level only.
        assert!(backlog_refuses(
            &mut state,
            7,
            NativeHandleKind::AHardwareBuffer,
            poll
        ));
        assert!(state.warned.backlog);

        // Once the fences fire the backlog drains and frames are admitted.
        fake.fire();
        assert_eq!(retired.load(Ordering::SeqCst), MAX_RETIRING as u64);
        assert!(!backlog_refuses(
            &mut state,
            7,
            NativeHandleKind::AHardwareBuffer,
            poll
        ));
    }

    #[test]
    fn a_poll_that_fires_a_fence_admits_the_frame() {
        let fake = FakeFence::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicU64::new(0));
        for _ in 0..MAX_RETIRING {
            retire_counting(&fake, &pending, &retired, 1);
        }
        assert!(backlog_full(&pending, || {}));
        assert!(
            !backlog_full(&pending, || fake.fire()),
            "the poll ran the completed fences, so there is room again"
        );
        assert_eq!(pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_device_lost_mid_chain_still_releases_once() {
        let fake = FakeFence::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicU64::new(0));
        retire_counting(&fake, &pending, &released, 2);
        fake.fire();
        assert_eq!(released.load(Ordering::SeqCst), 0, "one hop of two fired");
        // The device is lost: the second hop's callback is dropped unrun.
        fake.0.lock().unwrap().clear();
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(pending.load(Ordering::SeqCst), 0);
        fake.fire();
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_fence_dropped_unrun_still_releases() {
        let fake = FakeFence::default();
        let pending = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicU64::new(0));
        retire_counting(&fake, &pending, &released, 1);
        // The parked callbacks are dropped without ever running.
        fake.0.lock().unwrap().clear();
        assert_eq!(released.load(Ordering::SeqCst), 1);
        assert_eq!(pending.load(Ordering::SeqCst), 0);
    }
}
