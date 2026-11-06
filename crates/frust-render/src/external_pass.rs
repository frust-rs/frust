//! The seam an app — or a crate sitting beside the facade, a 3D renderer
//! being the motivating one — records its own GPU work through, ahead of the
//! engine's scene pass and into the engine's own frame encoder.
//!
//! # What this closes
//!
//! `frust_engine::EngineRenderer` has always been able to draw a texture it
//! did not create: `bind_texture` registers a `wgpu::TextureView` under a
//! [`SceneTextureId`], and a display list's
//! `Command::SceneTexture`/`SceneBuilder::scene_texture` naming that id
//! composites it. Nothing outside `frust-engine` could reach that, though —
//! [`crate::SurfaceRenderer`] owns the engine, and the frame's
//! `wgpu::CommandEncoder` is created and submitted inside one call — so the
//! only in-tree user of the registry was the engine's own shader-quad
//! pre-pass. This module is the reachable half: a process-wide registry of
//! caller-supplied [`ExternalPass`]es, each handed the live frame
//! ([`ExternalFrame`]) once per frame, before anything of the scene's own is
//! recorded.
//!
//! # The frame a pass is handed
//!
//! [`ExternalPass::record`] runs on the render thread (see "Threading"
//! below), inside the renderer's `submit`, into the *same*
//! `wgpu::CommandEncoder` the scene pass is about to be recorded into and
//! ahead of the engine's own shader-quad pre-pass.
//! That single-encoder ordering is the whole point, and it is the same one
//! the shader-quad pass relies on: a texture bound during `record` is already
//! registered when the display list naming it is compiled, and the passes
//! that write it are already recorded ahead of the pass that samples it. One
//! encoder gives both orderings for free, with no second submit and no fence.
//!
//! A pass records only — it never submits. That is [`crate::SurfaceRenderer`]'s
//! job, once, at the end of the frame; a pass that submits the frame's encoder
//! cannot (the encoder is only borrowed) and one that submits work of its own
//! on its own encoder breaks the ordering it came here for. A pass never
//! shares the frame's own depth attachment — it renders into attachments it
//! owns, on a target it owns, sized however that target needs to be.
//! `frust_gpu::encoder`'s two depth caller rules (clear ownership, and
//! matching comparison/direction) are for a *host* sharing one depth buffer
//! across renderers of its own; they have nothing to do with a pass
//! registered here, which the frame's depth attachment is never handed to.
//!
//! # Binding, and what the engine does with it
//!
//! [`ExternalFrame::bind_texture`] forwards to the engine's own registry, so
//! a pass renders into a target *it* owns and then hands the engine a view of
//! it. Re-binding the same id replaces the previous view (the engine's own
//! semantics), which is what lets a pass that re-creates its target on resize
//! keep one stable id. The composited result is *blended* unless the pass
//! binds with [`ExternalFrame::bind_texture_with`] and asserts opacity — the
//! engine never reads the caller's texels, so it cannot know
//! (`docs/LIMITATIONS.md`'s `engine-scene-texture-always-blended`).
//!
//! Binding happens inside `record`, immediately — it does not wait for the
//! frame to actually submit. So work a pass recorded is submitted only if the
//! frame is (a refused frame's encoder is dropped unsubmitted, taking every
//! command a pass recorded with it), but the bind side effect already landed
//! in the engine's registry and survives the refusal regardless: the next
//! *accepted* frame composites whatever view was bound, whether or not the
//! work that was meant to fill it ever ran.
//!
//! On a hand-over (one pass unregisters and a successor registers under the
//! same id), the binding the predecessor left behind is NOT inherited: every
//! drain notices the id now names a different registration and unbinds it
//! before any pass runs, so an id between owners draws nothing until its new
//! owner binds a view.
//!
//! # Lifetime of a registration
//!
//! A pass persists until [`unregister_external_pass`] takes it: the registry
//! is iterated every frame, never drained. Registration is process-wide —
//! one registration is recorded on every engine-tier surface that drains —
//! but the bookkeeping of what each surface's engine has bound is not: it
//! lives in that surface's own [`ExternalPassDrain`], which remembers, per
//! id, which registration it last ran there. Each drain *reconciles* that
//! record against the registry before it runs any pass: an id the registry
//! no longer holds, or holds under a different registration, has its engine
//! binding cleared first. So no engine keeps a view alive for an id whose
//! owner is gone, however many surfaces are live, and none of them depends
//! on another having drained first.
//!
//! Unregistering is not a barrier. It removes the pass from the registry
//! under the lock, but a drain that already snapshotted the pass before that
//! call runs is still mid-flight against its own copy of the `Arc` and will
//! still call `record` on it once more — no drain that *starts* after
//! [`unregister_external_pass`] returns ever will. The unbind itself is not
//! immediate either: each surface performs it on the next frame it actually
//! drains, so it does not happen while that surface is idle — a caller that
//! needs the binding gone right now, rather than whenever the surface next
//! presents, has to make sure a frame gets requested. What a drain tracks is
//! bounded by the ids it has run passes under and not yet seen unregistered,
//! never by how many times any one of them is registered or unregistered.
//!
//! # Threading
//!
//! - `record` runs on the draining surface's render thread — the UI thread
//!   when a shell renders inline (`FRUST_NO_RENDER_THREAD`) — and may move
//!   between threads across frames (a surface rebuilt, a second surface).
//! - `record` is **never called concurrently for the same registration**:
//!   each registration carries its own lock, held only around its `record`,
//!   so two surfaces draining at once take turns on it. It *may* run
//!   concurrently with other registrations' passes.
//! - [`register_external_pass`]/[`unregister_external_pass`] may be called
//!   from any thread, `record` included. Locks never nest the other way: the
//!   registry lock is released before any `record` runs, and a drain holds
//!   one registration's lock at a time.
//! - A pass keeps cross-frame state behind its own interior mutability; state
//!   that belongs to one surface (a target sized to it, a per-surface frame
//!   count) is keyed by [`ExternalFrame::drain_id`], and device-owned
//!   resources are rebuilt when [`ExternalFrame::device_generation`] changes.
//!
//! # A panicking pass does not take the frame down
//!
//! `record` is called inside `catch_unwind`. A pass that panics is reported
//! (`warn!` on the first panic under an id with a given Arc identity, `debug!`
//! afterwards) and, *if* the id it panicked under still names the same
//! registration (nothing else registered under it in the meantime — a pass
//! handing its id to a successor inside its own `record` before panicking
//! leaves that successor alone), removed from the registry, and the drain
//! that caught the panic clears its own engine's binding at once; every other
//! surface's drain clears its own on its next frame, by reconciling. If a
//! successor already holds the id, the panic is still reported as a debug
//! message saying the predecessor panicked after handing over. Either way the
//! frame goes on to record the scene. This is the same no-panic posture the
//! engine holds on the FFI boundary. It is a debug/dev net rather than a
//! promise: the workspace's `release` profile is `panic = "abort"`, where the
//! process is gone before any guard runs, so the shipped contract is still
//! that a pass must not panic.
//!
//! # Who reaches this
//!
//! The registry itself is shell-agnostic — a plain process-wide map, with no
//! device, window or platform in it — but the drain lives on the engine
//! tier's frame path (`SurfaceRenderer`'s `TierBackend::Engine` arm, which
//! builds a fresh [`ExternalPassDrain`] beside every engine it builds), so a
//! registered pass records exactly where an engine-tier surface is presenting
//! frames and nowhere else. With two engine-tier surfaces live at once, every
//! pass records into both frames, binding into each surface's own engine, and
//! each surface unbinds its own engine's view when the pass goes away.
//! `HeadlessRenderer::render` runs no external passes at all;
//! `render_with_passes` runs exactly the passes it is handed (explicitly,
//! never touching the registry, with a per-renderer frame index); and
//! `render_draining_registry` drains this registry the way a surface does,
//! with the process-wide frame index.
//!
//! # A residual race
//!
//! A registered pass that panics is retired by the first surface to catch
//! it. A second surface that had already snapshotted the registry before that
//! retirement still records the pass once more for its in-flight frame. If
//! that record panics too, the second panic is caught but not reported (the
//! registration is already gone). Unwinding builds only (release is
//! `panic = "abort"`); no memory-safety impact.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use frust_engine::SceneTextureHints;
use frust_gpu::SceneTextureId;

/// Caller-supplied GPU work recorded into the frame ahead of the scene.
///
/// Registered with [`register_external_pass`] under a [`SceneTextureId`] the
/// caller minted (`SceneTextureId::mint`, or `Texture::as_scene_texture` when
/// it has a `frust_gpu::Texture` to mint from), and called once per frame
/// with that frame's live [`ExternalFrame`] until
/// [`unregister_external_pass`] takes it back.
///
/// `Send + Sync` because the registry is process-wide and the call happens on
/// the render thread, which is not the thread that registered the pass on any
/// shell that splits the two, and is not even one fixed thread: with two
/// engine-tier surfaces live at once, each drains on its own render thread.
/// What the drain does guarantee is that `record` is **never called
/// concurrently for the same registration** — two surfaces draining at once
/// take turns on it — while it may run concurrently with *other*
/// registrations' passes (see the module docs' "Threading"). `record` takes
/// `&self`, so a pass mutating state across frames owns its own interior
/// mutability; state that belongs to one surface is keyed by
/// [`ExternalFrame::drain_id`]. Unregistering is not a barrier: a `record`
/// already in flight when [`unregister_external_pass`] runs may complete once
/// more. `record` must not drive a drain itself (a surface frame or a
/// `HeadlessRenderer::render_draining_registry`): the turn it holds on its
/// own registration is not reentrant, so reaching that registration again
/// from inside `record` deadlocks.
pub trait ExternalPass: Send + Sync {
    /// Records this pass's work for one frame.
    ///
    /// Record passes and staged uploads only — never a submit, and never a
    /// `wgpu::RenderPass` left open across the return (see
    /// `frust_gpu::encoder`'s borrowing contract, which the frame's encoder
    /// is under for exactly the same reason). The shared encoder must be left
    /// finishable on every exit path: no open render pass AND balanced
    /// `push_debug_group`/`pop_debug_group` calls (an unbalanced group makes
    /// `encoder.finish()` fail with wgpu's MissingPop and loses the frame;
    /// the pass is retired so it self-heals next frame). Bind whatever this
    /// frame should composite through [`ExternalFrame::bind_texture`]; the
    /// engine draws it wherever the frame's display list names the id.
    fn record(&self, frame: &mut ExternalFrame<'_>);
}

/// The per-drain values an [`ExternalFrame`] reports beside its resources.
#[derive(Clone, Copy)]
pub(crate) struct FrameStamp {
    frame_index: u64,
    drain_id: u64,
    device_generation: u64,
}

/// The live frame handed to [`ExternalPass::record`]: the device and queue
/// behind it, the encoder every pass of this frame is recorded into, and the
/// engine's texture registry to bind results into.
///
/// Borrowed for the duration of one `record` call and never longer — nothing
/// here can be stashed across frames, which is what keeps a pass from
/// out-living the surface whose device it was handed. `wgpu::Device` and
/// `wgpu::Queue` are cheaply `Clone` (both `Arc`-backed underneath), so
/// [`Self::device`]/[`Self::queue`] returning a borrow does not itself stop a
/// pass from cloning one and stashing the owned handle past this call —
/// doing that is a contract violation regardless of whether the borrow
/// checker catches it: the shell that owns the real device may drop and
/// recreate it (surface loss, a GPU reset), and a clone a pass kept past
/// that point references a device that looks alive but is not the one
/// backing any future frame, so GPU work built against it fails or panics
/// rather than silently rebinding to the shell's current one.
/// [`Self::device_generation`] is how a pass notices that change.
///
/// Built fresh for *each* pass a drain runs, scoped to that one pass's own
/// registered id ([`Self::id`]) — a pass never sees another pass's id, and
/// [`Self::bind_texture`]/[`Self::unbind_texture`] act only on its own.
pub struct ExternalFrame<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    encoder: &'a mut wgpu::CommandEncoder,
    /// The engine whose external-texture registry
    /// [`Self::bind_texture`]/[`Self::unbind_texture`] write. Deliberately
    /// not exposed: the engine's own frame API (`encode`, `end_frame`,
    /// `resize`) belongs to the renderer driving it, and a pass reaching it
    /// would be recording a second frame inside this one.
    engine: &'a mut frust_engine::EngineRenderer,
    /// The id the pass being recorded this call is registered under — what
    /// [`Self::id`]/[`Self::bind_texture`]/[`Self::unbind_texture`] act on.
    id: SceneTextureId,
    stamp: FrameStamp,
}

impl<'a> ExternalFrame<'a> {
    /// Wraps one pass's resources for one frame. Crate-private: an
    /// `ExternalFrame` is only ever built by [`run_external_passes`], fresh
    /// per pass, around a real in-flight frame.
    pub(crate) fn new(
        device: &'a wgpu::Device,
        queue: &'a wgpu::Queue,
        encoder: &'a mut wgpu::CommandEncoder,
        engine: &'a mut frust_engine::EngineRenderer,
        id: SceneTextureId,
        stamp: FrameStamp,
    ) -> Self {
        Self {
            device,
            queue,
            encoder,
            engine,
            id,
            stamp,
        }
    }

    /// The device this frame is being recorded on — the shell's own live
    /// device, not a second one.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        self.device
    }

    /// The queue the frame will be submitted on, for staged writes
    /// (`write_texture`, `write_buffer`) a pass needs committed before the
    /// frame reads them.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        self.queue
    }

    /// The frame's own encoder — the one the scene pass is recorded into
    /// after every pass has run, and the one the renderer submits once.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        self.encoder
    }

    /// The id the pass being recorded this call is registered under —
    /// what [`Self::bind_texture`]/[`Self::unbind_texture`] act on.
    #[must_use]
    pub fn id(&self) -> SceneTextureId {
        self.id
    }

    /// A frame counter, counting from zero. What it counts depends on the
    /// path that drained the pass:
    ///
    /// - **Surface frames and registry drains**
    ///   (`HeadlessRenderer::render_draining_registry` included): the count
    ///   is process-wide. It advances once per drain that has at least one
    ///   pass to run, so a pass animating off it sees a dense sequence
    ///   rather than one with the pass-free frames of other surfaces punched
    ///   out of it. With two engine-tier surfaces live, their drains share
    ///   the one sequence, so a pass needing a per-surface count keeps its
    ///   own keyed by [`Self::drain_id`].
    /// - **Explicit headless renders** (`render_with_passes`): the count is
    ///   that renderer's own. It counts, from zero, the renderer's explicit
    ///   renders that ran at least one pass (a render with no passes does not
    ///   advance it), and reads no process-wide state, so the same sequence of
    ///   renders on a fresh renderer always observes the same indices.
    ///
    /// It is a frame *counter*, not a clock: a pass needing wall-clock time
    /// reads one.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.stamp.frame_index
    }

    /// Which surface engine this frame is recorded for: a process-unique,
    /// non-zero id that stays the same for every frame of one engine and
    /// changes whenever a shell builds a fresh one (a fresh surface, a new
    /// device).
    ///
    /// A pass that keeps state per surface — a target sized to it, a binding
    /// it has made there — keys that state on this.
    #[must_use]
    pub fn drain_id(&self) -> u64 {
        self.stamp.drain_id
    }

    /// The generation of the device this frame is recorded on — equal to
    /// that device's `frust_gpu::DeviceHandle::generation` on every shell
    /// frame.
    ///
    /// A pass that caches device-owned resources (textures, pipelines,
    /// buffers) rebuilds them when this differs from the generation they were
    /// built under, rather than using resources from a device that is gone.
    #[must_use]
    pub fn device_generation(&self) -> u64 {
        self.stamp.device_generation
    }

    /// Registers `view` under this pass's own id ([`Self::id`]) so this
    /// frame's `Command::SceneTexture` naming that id composites it,
    /// returning whatever was bound under it before.
    ///
    /// `size` is the view's extent in texels — the rectangle the display
    /// list's destination is mapped onto. It is a pass's own choice, read
    /// however its owner reads layout (a widget's own size, typically); this
    /// type carries no target extent of its own to hand back. `view` must be
    /// a non-array 2D view of a float-sampleable texture carrying
    /// `wgpu::TextureUsages::TEXTURE_BINDING`. A zero extent, or one past
    /// `u16::MAX` on either axis, registers nothing and leaves scenes naming
    /// the id drawing nothing; so does an id nothing ever bound. Re-binding
    /// replaces the previous view.
    pub fn bind_texture(
        &mut self,
        size: (u32, u32),
        view: wgpu::TextureView,
    ) -> Option<wgpu::TextureView> {
        self.engine.bind_texture(self.id, size, view)
    }

    /// [`Self::bind_texture`] with registration-side [`SceneTextureHints`]:
    /// how the texture is filtered when drawn scaled, and whether the caller
    /// guarantees it is fully opaque.
    ///
    /// The engine never reads the bound texels, so an `opaque` hint is taken
    /// on trust: assert it only when alpha is one everywhere in `view`. A view
    /// holding any alpha below one drawn as opaque occludes what it should
    /// blend over. Re-binding replaces the hints along with the view.
    /// [`Self::bind_texture`] is this with the default hints.
    pub fn bind_texture_with(
        &mut self,
        size: (u32, u32),
        view: wgpu::TextureView,
        hints: SceneTextureHints,
    ) -> Option<wgpu::TextureView> {
        self.engine.bind_texture_with(self.id, size, view, hints)
    }

    /// Whether the engine this frame is recorded for currently holds a view
    /// under this pass's own id ([`Self::id`]).
    ///
    /// A binding lives in the engine, not in the pass, and a shell builds a
    /// fresh engine whenever it installs a fresh surface (an Android resume, a
    /// surface-format change) on the very same device. A pass that binds only
    /// when its own content changes reads this to notice that the binding it
    /// made earlier is gone and bind its current view again.
    #[must_use]
    pub fn has_binding(&self) -> bool {
        self.engine.bound_texture(self.id).is_some()
    }

    /// Removes the view bound under this pass's own id ([`Self::id`]),
    /// returning it. A pass tearing its own target down mid-run does this
    /// itself; a pass that simply stops being registered has it done for it
    /// (see the module docs).
    pub fn unbind_texture(&mut self) -> Option<wgpu::TextureView> {
        self.engine.unbind_texture(self.id)
    }
}

/// One registered pass, beside the id it was registered under.
///
/// The id rides along with the pass because the maps below are keyed by its
/// raw `u64`: ordering the drain by that key is what makes the sequence
/// passes are recorded in — they share one encoder — the same on every frame
/// and every run, and a [`SceneTextureId`] is not itself ordered.
struct Registration {
    id: SceneTextureId,
    pass: Arc<dyn ExternalPass>,
    /// Process-unique serial of this registration, drawn from
    /// [`NEXT_REGISTRATION`]: what a drain's reconcile compares to tell "the
    /// same registration" from "a successor under the same id". A serial
    /// rather than the `Arc`'s address, because an address can be reused by
    /// a successor allocated after its predecessor was freed, and because
    /// re-registering the very same `Arc` is still a hand-over.
    serial: u64,
    /// Held around this registration's `record`, and nothing else: what
    /// keeps two surfaces draining at once from calling it concurrently.
    record_lock: Arc<Mutex<()>>,
}

/// A panic-report slot for one pass, alongside the identity of the pass that
/// panicked: store the Arc's address as a usize to recognize when the same
/// pass panics again versus when a fresh pass with a different id panics.
struct PanicReport {
    identity: usize,
}

/// The process-wide registry: the live passes, and which ids have already had
/// a panic or a reserved-namespace refusal reported. What each surface's
/// engine has bound is not here — it is each surface's own
/// [`ExternalPassDrain`].
struct Registry {
    passes: BTreeMap<u64, Registration>,
    /// Ids a panic has already been reported at `warn!` for, alongside the
    /// identity (Arc pointer) of the pass that panicked. If the same pass id
    /// panics again with the same Arc, it reports at `debug!` instead of
    /// `warn!`. If a different Arc binds to the same id, the warn-once flag
    /// is cleared and the new pass's first panic is a fresh `warn!` too.
    reported_panics: BTreeMap<u64, PanicReport>,
    /// Flag for whether a reserved shader-program namespace registration
    /// attempt has already been reported at `warn!` level. Warn once
    /// process-wide (the refusal reason is identical for every id), then
    /// report at `debug!` for any further attempts.
    reported_reserved: bool,
    /// The same warn-once flag for the texture-source namespace, kept apart so
    /// a caller tripping one reserved namespace still hears about the other.
    reported_texture_source: bool,
}

impl Registry {
    const fn new() -> Self {
        Self {
            passes: BTreeMap::new(),
            reported_panics: BTreeMap::new(),
            reported_reserved: false,
            reported_texture_source: false,
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// Frames drained so far — the source of [`ExternalFrame::frame_index`].
static FRAME_INDEX: AtomicU64 = AtomicU64::new(0);

/// The next [`ExternalPassDrain::drain_id`]. Starts at 1 so `0` is never a
/// drain.
static NEXT_DRAIN_ID: AtomicU64 = AtomicU64::new(1);

/// The next [`Registration::serial`].
static NEXT_REGISTRATION: AtomicU64 = AtomicU64::new(1);

/// The registry, with poison ignored: one pass's panic must not turn every
/// later frame's drain into a panic of its own, which is the opposite of what
/// the `catch_unwind` around `record` is for. (Nothing panics while the guard
/// is held — `record` is called after it is dropped — so poison here would
/// only ever be collateral.)
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The reserved namespace `id` falls in, if any: what the explicit path
/// refuses by. [`register_external_pass`] refuses the same two namespaces
/// with its own inline checks (it logs each refusal once, per namespace).
fn reserved_namespace(id: SceneTextureId) -> Option<&'static str> {
    if id.is_shader_program() {
        Some("shader-program")
    } else if id.is_texture_source() {
        Some("texture-source")
    } else {
        None
    }
}

/// Registers `pass` under `id`, to be recorded every frame until
/// [`unregister_external_pass`] takes it back.
///
/// Answers `false` and changes nothing when `id` already has a pass: a
/// registration is a claim on an id, and silently displacing another
/// component's pass would leave it registered from its own point of view and
/// never called. Unregister first to hand an id over deliberately.
///
/// Also answers `false` and changes nothing when `id` is inside a reserved
/// namespace: the shader-program one (`SceneTextureId::is_shader_program`) —
/// that range belongs to the engine's own offscreen shader-effect targets,
/// and a caller-registered pass claiming one would silently steal a shader
/// program's target out from under it — or the texture-source one
/// (`SceneTextureId::is_texture_source`), which belongs to
/// [`crate::texture_bus`]'s registrations and is reached only through
/// [`crate::texture_bus::register_texture_source`]. Reported at `warn!` the
/// first time a namespace is refused this way, `debug!` on every later
/// attempt, so a caller retrying the same doomed registration every frame
/// does not spam the log.
///
/// A registration landing on an id a predecessor held does not inherit the
/// predecessor's engine bindings: every drain unbinds them before the new
/// pass first records there. A successful registration also clears any panic
/// already reported for `id`, so a fresh pass's first panic is reported fresh
/// rather than silently downgraded by a predecessor's history.
///
/// Callable from any thread, including from inside a `record`.
pub fn register_external_pass(id: SceneTextureId, pass: Arc<dyn ExternalPass>) -> bool {
    let raw = id.get();
    if id.is_shader_program() {
        let should_warn = {
            let mut registry = registry();
            let should_warn = !registry.reported_reserved;
            if should_warn {
                registry.reported_reserved = true;
            }
            should_warn
        };
        if should_warn {
            log::warn!(
                "external pass registration for scene texture {raw} refused: the id is inside \
                 the reserved shader-program namespace (further attempts at this id are logged \
                 at debug level)"
            );
        } else {
            log::debug!(
                "external pass registration for scene texture {raw} refused: reserved \
                 shader-program namespace"
            );
        }
        return false;
    }
    if id.is_texture_source() {
        let should_warn = {
            let mut registry = registry();
            let should_warn = !registry.reported_texture_source;
            if should_warn {
                registry.reported_texture_source = true;
            }
            should_warn
        };
        if should_warn {
            log::warn!(
                "external pass registration for scene texture {raw} refused: the id is inside \
                 the reserved texture-source namespace, which only the texture bus registers \
                 (further attempts are logged at debug level)"
            );
        } else {
            log::debug!(
                "external pass registration for scene texture {raw} refused: reserved \
                 texture-source namespace"
            );
        }
        return false;
    }
    insert_pass(id, pass)
}

/// Registers a texture-bus pass under its own texture-source `id` — the one
/// route into that reserved namespace, taken only by
/// [`crate::texture_bus::register_texture_source`]. Same claim semantics as
/// [`register_external_pass`] otherwise.
pub(crate) fn register_texture_source_pass(
    id: SceneTextureId,
    pass: Arc<dyn ExternalPass>,
) -> bool {
    debug_assert!(
        id.is_texture_source(),
        "texture-bus ids carry their namespace"
    );
    insert_pass(id, pass)
}

/// The claim itself, past any namespace check: refuses an id that already has
/// a pass, clears a predecessor's reported panic, and inserts under a fresh
/// registration serial and record lock.
fn insert_pass(id: SceneTextureId, pass: Arc<dyn ExternalPass>) -> bool {
    let raw = id.get();
    let pass_identity = Arc::as_ptr(&pass) as *const () as usize;
    let mut registry = registry();
    if registry.passes.contains_key(&raw) {
        return false;
    }
    if registry
        .reported_panics
        .get(&raw)
        .is_some_and(|report| report.identity != pass_identity)
    {
        registry.reported_panics.remove(&raw);
    }
    registry.passes.insert(
        raw,
        Registration {
            id,
            pass,
            serial: NEXT_REGISTRATION.fetch_add(1, Ordering::Relaxed),
            record_lock: Arc::new(Mutex::new(())),
        },
    );
    true
}

/// Removes the pass registered under `id`, answering whether there was one.
/// Every surface that ran it clears the engine binding it left behind on that
/// surface's next drain.
///
/// Callable from any thread, including from inside a `record`. Not a barrier:
/// a `record` already in flight may complete once more.
pub fn unregister_external_pass(id: SceneTextureId) -> bool {
    registry().passes.remove(&id.get()).is_some()
}

/// One binding a drain tracks: the id a pass ran under on this drain's
/// engine, and the serial of the registration that ran it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tracked {
    id: SceneTextureId,
    registration: u64,
}

/// One surface engine's side of the external-pass seam: which registration
/// it last ran under each id, so it can clear the engine bindings of any id
/// whose owner went away — by reconciling against the process-wide registry
/// on each drain rather than through a queue only one surface would consume.
///
/// Built fresh beside every engine (its bindings die with that engine), and
/// handed to [`run_external_passes`] on every one of that engine's frames.
/// Hidden from the crate's documented surface for the same reason
/// [`run_external_passes`] is.
#[doc(hidden)]
#[derive(Debug)]
pub struct ExternalPassDrain {
    drain_id: u64,
    device_generation: u64,
    bound: BTreeMap<u64, Tracked>,
    /// A weak handle to each explicit pass this drain last ran, keyed by id.
    /// An explicit pass's identity is its `Arc` address; a `Weak` keeps that
    /// allocation alive (not the pass), so the address cannot be reused by a
    /// later pass while this drain still tracks the id.
    pinned: BTreeMap<u64, Weak<dyn ExternalPass>>,
    /// Explicit renders this drain has stamped so far: the source of
    /// [`ExternalFrame::frame_index`] on the explicit path, so it depends on
    /// nothing outside this drain.
    explicit_frames: u64,
}

impl ExternalPassDrain {
    /// A drain for a fresh engine on the device of `device_generation`
    /// (`frust_gpu::DeviceHandle::generation`), with a process-unique
    /// [`Self::drain_id`].
    #[must_use]
    pub fn new(device_generation: u64) -> Self {
        Self {
            drain_id: NEXT_DRAIN_ID.fetch_add(1, Ordering::Relaxed),
            device_generation,
            bound: BTreeMap::new(),
            pinned: BTreeMap::new(),
            explicit_frames: 0,
        }
    }

    /// What [`ExternalFrame::drain_id`] reports on this drain's frames.
    #[must_use]
    pub fn drain_id(&self) -> u64 {
        self.drain_id
    }

    /// What [`ExternalFrame::device_generation`] reports on this drain's
    /// frames.
    #[must_use]
    pub fn device_generation(&self) -> u64 {
        self.device_generation
    }
}

/// One pass a drain is about to run, snapshotted out of the registry.
struct Due {
    id: SceneTextureId,
    serial: u64,
    pass: Arc<dyn ExternalPass>,
    /// The registration's record lock; `None` for an explicit pass, which no
    /// other drain can see and so has nothing to serialize against.
    record_lock: Option<Arc<Mutex<()>>>,
}

/// The registry's passes, in id order, for a drain to run — or `None` when
/// there is nothing at all to do: no pass registered and no binding this
/// drain still tracks. The cheap path a process that registered nothing
/// takes every frame.
fn snapshot(registry: &Registry, drain: &ExternalPassDrain) -> Option<Vec<Due>> {
    if registry.passes.is_empty() && drain.bound.is_empty() {
        return None;
    }
    Some(
        registry
            .passes
            .values()
            .map(|registration| Due {
                id: registration.id,
                serial: registration.serial,
                pass: Arc::clone(&registration.pass),
                record_lock: Some(Arc::clone(&registration.record_lock)),
            })
            .collect(),
    )
}

/// The tracked bindings the registry no longer backs: every id in `bound`
/// that `live` (raw id → the serial registered under it now) does not name,
/// or names under a different registration. Pure, so the reconcile rule is
/// testable without an engine; allocates nothing when nothing is stale.
fn stale_bindings(
    bound: &BTreeMap<u64, Tracked>,
    live: impl Fn(u64) -> Option<u64>,
) -> Vec<SceneTextureId> {
    bound
        .iter()
        .filter(|(raw, tracked)| live(**raw) != Some(tracked.registration))
        .map(|(_, tracked)| tracked.id)
        .collect()
}

/// Runs one frame's external passes into `encoder`, after clearing the engine
/// bindings of every id `drain` tracked whose registration is gone.
///
/// Called by [`crate::SurfaceRenderer`] on the engine tier, immediately
/// before the shader-quad pre-pass and so before anything of the scene's own
/// is recorded, with the [`ExternalPassDrain`] it built beside `engine`.
/// Hidden from the crate's documented surface: it names the engine renderer
/// and the frame's raw `wgpu` resources, which are the renderer's to hold,
/// and it is reachable only so this crate's own GPU tests (and an offscreen
/// renderer of its own) can drive the identical code path against an engine
/// of their own — a surface, and so `submit`, is not something a headless
/// test can produce.
///
/// Cheap on a process that registered nothing: two `is_empty` checks under
/// one uncontended lock, and no encoder work at all.
#[doc(hidden)]
pub fn run_external_passes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    engine: &mut frust_engine::EngineRenderer,
    drain: &mut ExternalPassDrain,
) {
    // Snapshot under the lock and release it before any `record` runs: a pass
    // is caller code, free to register or unregister another pass (its own
    // included) from inside its `record`, which would deadlock against a held
    // guard.
    let Some(due) = snapshot(&registry(), drain) else {
        return;
    };
    for pass in run_due(device, queue, encoder, engine, drain, due, false) {
        if drop_panicking_pass(pass.id, pass.serial, &pass.pass) {
            // The surface that caught the panic clears its own binding at
            // once; every other one does on its next drain, by reconciling.
            drain.bound.remove(&pass.id.get());
            engine.unbind_texture(pass.id);
        }
    }
}

/// Runs an explicit list of passes into `encoder` on `drain`'s engine, for a
/// renderer that owns its passes rather than draining the process-wide
/// registry (an offscreen render handed its passes by the caller).
///
/// Nothing here touches the registry: an explicit pass has no registration,
/// so it is identified within `drain` by the address of its `Arc` (the caller
/// owns the list, no other drain sees it) and takes no record lock. Passes
/// run in ascending id order. Before any pass records, every id `drain`
/// tracks that is absent from `passes` — or present under a different `Arc`
/// — is unbound, so mixing explicit and registry renders on one engine never
/// leaves a stale bind.
///
/// A panicking pass is isolated exactly as a registered one is, but nothing
/// is retired (there is no registry entry to retire): its binding is cleared,
/// the remaining passes still record, and the panic is reported as an `Err`
/// naming the ids so the caller can fail the render.
///
/// # Errors
///
/// Fails, recording nothing, when an entry's id is inside a reserved
/// namespace (the same refusal [`register_external_pass`] makes) or when two
/// entries share an id; and after
/// recording when any pass panicked.
#[doc(hidden)]
pub fn run_explicit_external_passes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    engine: &mut frust_engine::EngineRenderer,
    drain: &mut ExternalPassDrain,
    passes: &[(SceneTextureId, Arc<dyn ExternalPass>)],
) -> Result<(), String> {
    if let Some((id, namespace)) = passes
        .iter()
        .find_map(|(id, _)| reserved_namespace(*id).map(|namespace| (*id, namespace)))
    {
        return Err(format!(
            "scene texture {} is inside the reserved {namespace} namespace and cannot be \
             named by an explicit external pass",
            id.get()
        ));
    }
    let mut due: Vec<Due> = passes
        .iter()
        .map(|(id, pass)| Due {
            id: *id,
            // An explicit pass's identity is its allocation address. It lives
            // in the same `u64` space as registration serials (small
            // counters), which cannot collide with a real address, and
            // `drain.pinned` keeps every tracked explicit allocation alive,
            // so an address is never reused while this drain still tracks it.
            serial: Arc::as_ptr(pass) as *const () as usize as u64,
            pass: Arc::clone(pass),
            record_lock: None,
        })
        .collect();
    due.sort_by_key(|pass| pass.id.get());
    if let Some(pair) = due.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(format!(
            "scene texture {} is named by more than one explicit external pass",
            pair[0].id.get()
        ));
    }
    let panicked = run_due(device, queue, encoder, engine, drain, due, true);
    // Every id tracked now came from `passes` (reconcile unbound the rest), so
    // pinning exactly this list keeps each tracked identity unique.
    drain.pinned = passes
        .iter()
        .map(|(id, pass)| (id.get(), Arc::downgrade(pass)))
        .collect();
    for pass in &panicked {
        drain.bound.remove(&pass.id.get());
        engine.unbind_texture(pass.id);
    }
    if panicked.is_empty() {
        Ok(())
    } else {
        let ids: Vec<String> = panicked.iter().map(|p| p.id.get().to_string()).collect();
        Err(format!(
            "external pass for scene texture {} panicked; the frame was recorded without it",
            ids.join(", ")
        ))
    }
}

/// The shared core of both drains: reconciles `drain` against `due` (id
/// order), then records each pass under panic isolation, and answers the
/// passes that panicked for the caller to retire or report. `explicit`
/// selects the frame index: the drain's own counter, or the process-wide one.
fn run_due(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    engine: &mut frust_engine::EngineRenderer,
    drain: &mut ExternalPassDrain,
    due: Vec<Due>,
    explicit: bool,
) -> Vec<Due> {
    // Before any pass records: an id with no owner — or a new one — keeps no
    // view of a previous owner's alive in this engine. `due` is in id order,
    // so the lookup is a binary search.
    let stale = stale_bindings(&drain.bound, |raw| {
        due.binary_search_by_key(&raw, |pass| pass.id.get())
            .ok()
            .map(|index| due[index].serial)
    });
    for id in stale {
        drain.bound.remove(&id.get());
        engine.unbind_texture(id);
    }
    if due.is_empty() {
        return Vec::new();
    }

    let frame_index = if explicit {
        let index = drain.explicit_frames;
        drain.explicit_frames += 1;
        index
    } else {
        FRAME_INDEX.fetch_add(1, Ordering::Relaxed)
    };
    let stamp = FrameStamp {
        frame_index,
        drain_id: drain.drain_id,
        device_generation: drain.device_generation,
    };
    let mut panicked = Vec::new();
    for pass in due {
        // Tracked before `record` runs, so whatever it binds — even on a path
        // that panics afterwards — is this drain's to clear once the
        // registration is gone.
        drain.bound.insert(
            pass.id.get(),
            Tracked {
                id: pass.id,
                registration: pass.serial,
            },
        );
        let failed = {
            // One registration's `record` at a time, process-wide: the only
            // lock held across caller code, and never alongside another one
            // (the registry's is released by the caller, and this one is
            // dropped before the next pass's is taken). Poison is ignored:
            // the `catch_unwind` below returns before the guard drops, so a
            // panic in `record` never poisons it, and an unrelated poison
            // must not stop the pass running.
            let _serialized = pass
                .record_lock
                .as_ref()
                .map(|lock| lock.lock().unwrap_or_else(PoisonError::into_inner));
            // Built fresh per pass, scoped to that pass's own id — `device`,
            // `encoder` and `engine` are reborrowed each iteration rather
            // than moved, so the outer `&mut` references stay usable for the
            // next pass once this one's `ExternalFrame` goes out of scope.
            let mut frame =
                ExternalFrame::new(device, queue, &mut *encoder, &mut *engine, pass.id, stamp);
            // A pass is caller code on the render thread: unwinding out of it
            // would abandon the frame's encoder mid-recording and take the
            // surface's whole frame loop with it.
            catch_unwind(AssertUnwindSafe(|| pass.pass.record(&mut frame))).is_err()
        };
        if failed {
            panicked.push(pass);
        }
    }
    panicked
}

/// Retires the registration `serial` under `id` after its pass panicked,
/// *if* it is still the one registered there: report it once and take it out
/// of the registry, answering whether it was retired.
///
/// `serial` and `pass` are what [`run_external_passes`] snapshotted before
/// calling `record` — compared against whatever is registered under `id`
/// right now rather than blindly removed by id. A pass is caller code, free
/// to hand its own id to a successor (unregister, then register a
/// replacement) from inside the very `record` call that then panics; retiring
/// by id alone would tear that successor out along with the pass that
/// actually failed, even though it was never in the unwind at all. The panic
/// is still reported either way — it happened regardless of what is
/// registered under `id` now — but the registry itself is only touched when
/// the serial check confirms nothing has taken the id over since.
fn drop_panicking_pass(id: SceneTextureId, serial: u64, pass: &Arc<dyn ExternalPass>) -> bool {
    let raw = id.get();
    let pass_identity = Arc::as_ptr(pass) as *const () as usize;

    let (still_registered, should_warn, was_different_pass) = {
        let mut registry = registry();
        let registration = registry.passes.get(&raw);
        let still_registered = registration.is_some_and(|r| r.serial == serial);
        let was_different_pass = registration.is_some_and(|r| r.serial != serial);

        let should_warn = registry
            .reported_panics
            .get(&raw)
            .is_none_or(|report| report.identity != pass_identity);

        if still_registered {
            registry.passes.remove(&raw);
        }

        if should_warn {
            registry.reported_panics.insert(
                raw,
                PanicReport {
                    identity: pass_identity,
                },
            );
        }

        (still_registered, should_warn, was_different_pass)
    };

    if was_different_pass {
        log::debug!(
            "external pass for scene texture {raw} panicked after handing over; \
             the successor stays registered"
        );
    } else if still_registered && should_warn {
        log::warn!(
            "external pass for scene texture {raw} panicked and was unregistered; \
             the frame was recorded without it (further panics of this id are \
             logged at debug level)"
        );
    } else if still_registered {
        log::debug!("external pass for scene texture {raw} panicked and was unregistered");
    }
    still_registered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the cases: the registry is process-wide, so two cases
    /// registering at once would see each other's passes. Poison is ignored
    /// deliberately — one failing case must not cascade into every sibling.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A pass that records nothing — enough for every claim about the
    /// registry itself, which never touches a GPU.
    struct Inert;

    impl ExternalPass for Inert {
        fn record(&self, _frame: &mut ExternalFrame<'_>) {}
    }

    /// The serial registered under `id` right now, if anything is.
    fn registration(id: SceneTextureId) -> Option<u64> {
        registry().passes.get(&id.get()).map(|r| r.serial)
    }

    /// A `bound` map tracking each `(id, serial)` pair.
    fn tracking(pairs: &[(SceneTextureId, u64)]) -> BTreeMap<u64, Tracked> {
        pairs
            .iter()
            .map(|&(id, registration)| (id.get(), Tracked { id, registration }))
            .collect()
    }

    #[test]
    fn reserved_namespaces_are_named_and_minted_ids_are_not() {
        assert_eq!(
            reserved_namespace(SceneTextureId::for_shader_program(7)),
            Some("shader-program")
        );
        assert_eq!(
            reserved_namespace(SceneTextureId::for_texture_source(7)),
            Some("texture-source")
        );
        assert_eq!(reserved_namespace(SceneTextureId::mint()), None);
    }

    #[test]
    fn registering_an_id_twice_keeps_the_first_pass() {
        let _serial = serial();
        let id = SceneTextureId::mint();

        assert!(register_external_pass(id, Arc::new(Inert)));
        let first = registration(id);
        assert!(
            !register_external_pass(id, Arc::new(Inert)),
            "a second registration must not displace the first"
        );
        assert_eq!(registration(id), first, "the first registration stays");

        assert!(unregister_external_pass(id));
        assert_eq!(registration(id), None);
    }

    #[test]
    fn a_texture_source_id_is_refused_to_an_ordinary_pass_but_open_to_the_bus() {
        let _serial = serial();
        let id = SceneTextureId::for_texture_source(SceneTextureId::mint().get());

        assert!(
            !register_external_pass(id, Arc::new(Inert)),
            "the texture-source namespace belongs to the bus"
        );
        assert_eq!(registration(id), None);
        assert!(registry().reported_texture_source);

        assert!(
            register_texture_source_pass(id, Arc::new(Inert)),
            "the bus's own route claims the id"
        );
        assert!(
            !register_texture_source_pass(id, Arc::new(Inert)),
            "a claimed id is still a claim on the bus's route"
        );
        assert!(unregister_external_pass(id));
    }

    #[test]
    fn unregistering_an_unregistered_id_answers_false() {
        let _serial = serial();
        let id = SceneTextureId::mint();

        assert!(!unregister_external_pass(id));
        assert_eq!(registration(id), None);
    }

    #[test]
    fn re_registering_an_id_is_a_new_registration_even_for_the_same_arc() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass: Arc<dyn ExternalPass> = Arc::new(Inert);

        assert!(register_external_pass(id, Arc::clone(&pass)));
        let first = registration(id).expect("registered");
        assert!(unregister_external_pass(id));
        assert!(register_external_pass(id, Arc::clone(&pass)));
        let second = registration(id).expect("registered again");
        assert_ne!(
            first, second,
            "a hand-over is a new registration, so every drain unbinds the predecessor's view"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn a_panicking_pass_is_retired() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass: Arc<dyn ExternalPass> = Arc::new(Inert);

        assert!(register_external_pass(id, Arc::clone(&pass)));
        let serial = registration(id).expect("registered");
        assert!(
            drop_panicking_pass(id, serial, &pass),
            "the registration that panicked is retired"
        );
        assert_eq!(
            registration(id),
            None,
            "a panicking pass leaves the registry exactly as an unregistered one does"
        );

        // The second retirement reports at debug rather than warn; what is
        // checked here is that reporting twice is not itself a panic and
        // leaves the state alone.
        assert!(!drop_panicking_pass(id, serial, &pass));
        assert_eq!(registration(id), None);
    }

    #[test]
    fn retiring_a_panic_leaves_a_successor_registered_under_the_same_id_alone() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let original: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&original)));
        let original_serial = registration(id).expect("registered");

        // What a pass handing its own id to a replacement inside `record`
        // does before it panics: unregister, then register the successor —
        // both while the drain's snapshot still holds `original`.
        assert!(unregister_external_pass(id));
        let successor: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&successor)));
        let successor_serial = registration(id).expect("successor registered");

        // The drain retires by comparing the snapshot it took against
        // whatever is registered now, not by id alone.
        assert!(!drop_panicking_pass(id, original_serial, &original));
        assert_eq!(
            registration(id),
            Some(successor_serial),
            "the successor is left registered"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn retiring_a_panic_spares_a_successor_registering_the_very_same_arc() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&pass)));
        let original_serial = registration(id).expect("registered");

        // A hand-over to the same `Arc`: an address comparison could not
        // tell the two registrations apart, a serial does.
        assert!(unregister_external_pass(id));
        assert!(register_external_pass(id, Arc::clone(&pass)));
        let successor_serial = registration(id).expect("re-registered");

        assert!(!drop_panicking_pass(id, original_serial, &pass));
        assert_eq!(registration(id), Some(successor_serial));

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn a_fresh_registration_with_a_different_arc_clears_the_reported_panic_flag() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass1: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&pass1)));

        drop_panicking_pass(id, registration(id).expect("registered"), &pass1);
        assert!(
            registry().reported_panics.contains_key(&id.get()),
            "the first panic of this id is recorded as already reported"
        );

        let pass2: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&pass2)));
        assert!(
            !registry().reported_panics.contains_key(&id.get()),
            "registering a different Arc clears the flag, so a new pass's first panic warns again"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn re_registering_the_same_arc_keeps_the_reported_panic_flag() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&pass)));

        drop_panicking_pass(id, registration(id).expect("registered"), &pass);
        assert!(
            registry().reported_panics.contains_key(&id.get()),
            "the first panic is recorded"
        );

        // Re-register the same Arc: the panic flag stays since it's the same
        // identity.
        assert!(register_external_pass(id, Arc::clone(&pass)));
        assert!(
            registry().reported_panics.contains_key(&id.get()),
            "re-registering the same Arc keeps the panic flag (no warn on next panic)"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn drains_have_distinct_non_zero_ids_and_keep_their_generation() {
        let a = ExternalPassDrain::new(7);
        let b = ExternalPassDrain::new(7);
        assert_ne!(a.drain_id(), 0);
        assert_ne!(a.drain_id(), b.drain_id());
        assert_eq!(a.device_generation(), 7);
        assert!(a.bound.is_empty(), "a fresh drain tracks nothing");
    }

    #[test]
    fn reconcile_unbinds_an_id_gone_from_the_registry() {
        let gone = SceneTextureId::mint();
        let bound = tracking(&[(gone, 3)]);
        assert_eq!(stale_bindings(&bound, |_| None), vec![gone]);
    }

    #[test]
    fn reconcile_unbinds_an_id_whose_registration_changed() {
        let id = SceneTextureId::mint();
        let mut bound = tracking(&[(id, 3)]);
        let live = BTreeMap::from([(id.get(), 4)]);

        let stale = stale_bindings(&bound, |raw| live.get(&raw).copied());
        assert_eq!(stale, vec![id], "a successor never inherits the binding");

        // What the drain does next: drop the stale entry, then re-track the
        // id under the registration it runs now — after which it is current.
        for stale in stale {
            bound.remove(&stale.get());
        }
        bound.insert(
            id.get(),
            Tracked {
                id,
                registration: 4,
            },
        );
        assert!(stale_bindings(&bound, |raw| live.get(&raw).copied()).is_empty());
    }

    #[test]
    fn reconcile_leaves_unchanged_ids_alone() {
        let kept = SceneTextureId::mint();
        let gone = SceneTextureId::mint();
        let bound = tracking(&[(kept, 1), (gone, 2)]);
        let live = BTreeMap::from([(kept.get(), 1)]);

        assert_eq!(
            stale_bindings(&bound, |raw| live.get(&raw).copied()),
            vec![gone],
            "only the id whose registration went away is unbound"
        );
    }

    #[test]
    fn an_empty_registry_and_an_empty_drain_do_no_work() {
        let registry = Registry::new();
        let drain = ExternalPassDrain::new(1);
        assert!(
            snapshot(&registry, &drain).is_none(),
            "nothing registered and nothing tracked is the early return"
        );
        let stale = stale_bindings(&BTreeMap::new(), |_| None);
        assert!(stale.is_empty());
        assert_eq!(stale.capacity(), 0, "nothing stale allocates nothing");
    }

    #[test]
    fn a_drain_still_tracking_a_binding_reconciles_against_an_empty_registry() {
        let registry = Registry::new();
        let id = SceneTextureId::mint();
        let mut drain = ExternalPassDrain::new(1);
        drain.bound = tracking(&[(id, 9)]);

        let due = snapshot(&registry, &drain).expect("a tracked binding is work to do");
        assert!(due.is_empty(), "no pass to run");
        assert_eq!(stale_bindings(&drain.bound, |_| None), vec![id]);
    }
}
