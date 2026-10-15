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
//! Every frame's [`SourceFrame::release`] callback runs exactly once, as soon
//! as the bus is done with the payload: after the upload is enqueued, after
//! the importer returned, or right away when the frame is refused. An
//! importer that needs the platform buffer alive for longer than its own call
//! keeps its own reference (in the imported texture's lifetime, typically).
//!
//! The returned [`TextureRegistration`] reports the id to draw by
//! ([`TextureRegistration::scene_texture_id`], what a widget's
//! `draw_scene_texture` names) and unregisters on drop.
//!
//! # Why a closure, not a plugin type
//!
//! The renderer never depends on the plugin substrate, and the substrate never
//! names the renderer, so neither can hand the other its frame type. The
//! types here are this crate's own plain-data twins of the substrate's
//! producer-side ones, with identical field names; the bridge is a closure
//! written by code that depends on both (a plugin's facade-side half, or the
//! app):
//!
//! ```ignore
//! let registration = register_texture_source(move || {
//!     sink.take_latest().map(|f| SourceFrame {
//!         width: f.width,
//!         height: f.height,
//!         format: SourceFormat::Rgba8,
//!         payload: match f.payload {
//!             FramePayload::Cpu(pixels) => SourcePayload::Cpu(pixels),
//!             FramePayload::Native(h) => SourcePayload::Native(NativeHandle {
//!                 kind: /* the matching NativeHandleKind */,
//!                 ptr: h.ptr,
//!             }),
//!         },
//!         release: f.release,
//!     })
//! });
//! ```
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

use std::ffi::c_void;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use frust_gpu::SceneTextureId;

use crate::external_pass::{
    ExternalFrame, ExternalPass, register_texture_source_pass, unregister_external_pass,
};

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
/// The bus never dereferences `ptr`; only the importer installed for `kind`
/// does. The frame's producer owns the buffer's lifetime and drops its own
/// reference in the frame's release callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeHandle {
    /// The platform object `ptr` points at.
    pub kind: NativeHandleKind,
    /// The platform object itself.
    pub ptr: *mut c_void,
}

// SAFETY: an inert address plus a tag — moving it between threads moves only
// the address, and nothing here dereferences it. Both kinds it can name are
// reference-counted platform objects that are safe to retain, release and
// import from any thread, and the frame carrying the handle also carries the
// producer's own reference (released by its callback), so the object outlives
// the address's trip to the render thread.
unsafe impl Send for NativeHandle {}

/// Where a [`SourceFrame`]'s pixels live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourcePayload {
    /// Pixels in CPU memory, laid out per the frame's [`SourceFormat`].
    Cpu(Arc<[u8]>),
    /// A platform GPU buffer, imported rather than uploaded.
    Native(NativeHandle),
}

/// The callback a frame carries to tell its producer the bus is done with it.
pub type ReleaseCallback = Box<dyn FnOnce() + Send>;

/// One frame a source hands the bus.
///
/// Plain public fields with the same names as the producer-side frame type,
/// so a bridge moves them across one for one. Dropping a frame does not run
/// `release`: the bus runs it for every frame it is handed (see the module
/// docs), and a holder that drops a frame without passing it on calls
/// [`Self::release`].
pub struct SourceFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How `payload`'s pixels are laid out.
    pub format: SourceFormat,
    /// The pixels, or the platform buffer holding them.
    pub payload: SourcePayload,
    /// Run exactly once when the bus is done with the frame.
    pub release: Option<ReleaseCallback>,
}

impl SourceFrame {
    /// A CPU frame of tightly packed premultiplied RGBA8 with no release
    /// callback.
    #[must_use]
    pub fn rgba8(width: u32, height: u32, pixels: Arc<[u8]>) -> Self {
        Self {
            width,
            height,
            format: SourceFormat::Rgba8,
            payload: SourcePayload::Cpu(pixels),
            release: None,
        }
    }

    /// Attaches the callback run once the bus is done with the frame,
    /// replacing any callback already attached (which is dropped unrun).
    #[must_use]
    pub fn on_release(mut self, release: impl FnOnce() + Send + 'static) -> Self {
        self.release = Some(Box::new(release));
        self
    }

    /// Runs the release callback, if any, consuming the frame.
    pub fn release(mut self) {
        if let Some(release) = self.release.take() {
            release();
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
    /// Imports `handle` on `device`. The bus runs the frame's release callback
    /// as soon as this returns, so an importer whose texture aliases the
    /// platform buffer keeps its own reference to it for the texture's
    /// lifetime.
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
/// mailbox), never block for one. A source that panics is retired like any
/// panicking external pass; the frame it was producing is dropped unreleased.
pub fn register_texture_source(
    source: impl FnMut() -> Option<SourceFrame> + Send + 'static,
) -> TextureRegistration {
    let id = SceneTextureId::for_texture_source(NEXT_SOURCE.fetch_add(1, Ordering::Relaxed));
    let status = Arc::new(Status::default());
    let pass = SourcePass {
        state: Mutex::new(PassState {
            source: Box::new(source),
            shown: None,
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
}

/// Which refusals a registration has already warned about; later ones of the
/// same kind are logged at debug level.
#[derive(Default)]
struct Warned {
    malformed: bool,
    missing_importer: bool,
    import_failed: bool,
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
            state.shown = None;
        }

        let mut rebind = false;
        if let Some(source_frame) = (state.source)() {
            let SourceFrame {
                width,
                height,
                format,
                payload,
                release,
            } = source_frame;
            rebind = match payload {
                SourcePayload::Cpu(pixels) => {
                    upload(state, frame, (width, height), format, &pixels)
                }
                SourcePayload::Native(handle) => import(state, frame, &handle),
            };
            if let Some(release) = release {
                release();
            }
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
        state.shown = Some(Shown {
            device: frame.device().clone(),
            view,
            size,
            upload: Some(texture),
        });
    }
    changed
}

/// Imports one native frame through the importer installed for its kind,
/// answering whether the shown view changed. Refused (and warned about once)
/// when no importer is installed or the import fails; the last good frame
/// stays shown either way.
fn import(state: &mut PassState, frame: &mut ExternalFrame<'_>, handle: &NativeHandle) -> bool {
    let id = frame.id().get();
    let Some(importer) = native_importer(handle.kind) else {
        report(
            &mut state.warned.missing_importer,
            id,
            format_args!(
                "refused a {:?} frame: no native importer is installed for that kind",
                handle.kind
            ),
        );
        return false;
    };
    match importer.import(handle, frame.device(), frame.queue()) {
        Ok(imported) => {
            state.shown = Some(Shown {
                device: frame.device().clone(),
                view: imported.view,
                size: imported.size,
                upload: None,
            });
            true
        }
        Err(error) => {
            report(
                &mut state.warned.import_failed,
                id,
                format_args!("refused a {:?} frame: {error}", handle.kind),
            );
            false
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
}
