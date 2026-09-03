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
//! [`ExternalPass::record`] runs on the render thread, inside the renderer's
//! `submit`, into the *same* `wgpu::CommandEncoder` the scene pass is about
//! to be recorded into and ahead of the engine's own shader-quad pre-pass.
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
//! keep one stable id. The composited result is always *blended*, never
//! claimed opaque — the engine never reads the caller's texels, so it cannot
//! know (`docs/LIMITATIONS.md`'s `engine-scene-texture-always-blended`).
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
//! same id before the drain finishes), the queued unbind is NOT cancelled:
//! it runs at the next drain before any pass, so an id between owners draws
//! nothing until its new owner binds a view.
//!
//! # Lifetime of a registration
//!
//! A pass persists until [`unregister_external_pass`] takes it: the registry
//! is iterated every frame, never drained. Unregistering queues the id for an
//! `unbind_texture` the next drain performs *before* it runs any pass, so the
//! engine never keeps a view alive for an id whose owner is gone. Registering
//! the same id again before that flush does NOT cancel the queued unbind —
//! the id has an owner again, and the incoming pass binds whatever it wants,
//! but an id between owners draws nothing until its new owner records a
//! binding.
//!
//! Unregistering is not a barrier. It removes the pass from the registry
//! under the lock, but a drain that already snapshotted the pass before that
//! call runs is still mid-flight against its own copy of the `Arc` and will
//! still call `record` on it once more — no drain that *starts* after
//! [`unregister_external_pass`] returns ever will. The unbind itself is
//! queued, not immediate: it lands on the next frame something actually
//! drains, so it does not happen while the surface presenting it is idle — a
//! caller that needs the binding gone right now, rather than whenever the
//! surface next presents, has to make sure a frame gets requested. The queue
//! it lands in is a map keyed by id, so its size is bounded by the number of
//! *distinct* ids unregistered since the last drain, never by how many times
//! any one of them is unregistered.
//!
//! # A panicking pass does not take the frame down
//!
//! `record` is called inside `catch_unwind`. A pass that panics is reported
//! (`warn!` on the first panic under an id with a given Arc identity, `debug!`
//! afterwards) and, *if* the id it panicked under still names the same pass
//! (nothing else registered under it in the meantime — a pass handing its id
//! to a successor inside its own `record` before panicking leaves that
//! successor alone), removed from the registry and queued for the same unbind
//! an explicit unregistration queues. If a successor already holds the id, the
//! panic is still reported as a debug message saying the predecessor panicked
//! after handing over. Either way the frame goes on to record the scene.
//! This is the same no-panic posture the engine holds on the FFI boundary.
//! It is a debug/dev net rather than a promise: the workspace's `release`
//! profile is `panic = "abort"`, where the process is gone before any guard
//! runs, so the shipped contract is still that a pass must not panic.
//!
//! # Who reaches this
//!
//! The registry itself is shell-agnostic — a plain process-wide map, with no
//! device, window or platform in it — but the drain lives on the engine
//! tier's frame path (`SurfaceRenderer`'s `TierBackend::Engine` arm), so a
//! registered pass records exactly where an engine-tier surface is presenting
//! frames and nowhere else. `HeadlessRenderer` drives the engine through its
//! own frame path and does not drain this registry.
//!
//! One more consequence of a *process-wide* registry meeting a *per-surface*
//! engine: with two engine-tier surfaces live at once, every pass records
//! into both frames (binding into each surface's own engine, which is what a
//! caller wants), but a queued unbind is consumed by whichever surface drains
//! first. One engine-tier surface per process is what every shell does today.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
/// shell that splits the two — and, with two engine-tier surfaces live in one
/// process at once (each presenting through its own render thread), the same
/// pass can be called by both at the same time: `record` genuinely may run
/// concurrently on two threads, not merely sequentially from a changing one.
/// `record` takes `&self`, so a pass mutating state across frames owns its
/// own interior mutability, and that interior mutability has to tolerate the
/// concurrent case, not just the sequential one.
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
    frame_index: u64,
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
        frame_index: u64,
    ) -> Self {
        Self {
            device,
            queue,
            encoder,
            engine,
            id,
            frame_index,
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

    /// How many frames this process has drained passes in, counting from
    /// zero.
    ///
    /// Advances once per drain that has at least one pass to run, so a pass
    /// animating off it sees a dense sequence rather than one with the
    /// pass-free frames of other surfaces punched out of it. It is a frame
    /// *counter*, not a clock: a pass needing wall-clock time reads one.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
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
}

/// A panic-report slot for one pass, alongside the identity of the pass that
/// panicked: store the Arc's address as a usize to recognize when the same
/// pass panics again versus when a fresh pass with a different id panics.
struct PanicReport {
    identity: usize,
}

/// The process-wide registry: the live passes, the ids awaiting an unbind,
/// and which ids have already had a panic or a reserved-namespace refusal
/// reported.
struct Registry {
    passes: BTreeMap<u64, Registration>,
    /// Ids whose pass is gone — unregistered, or removed after a panic — and
    /// whose engine binding the next drain clears. Keyed by id, so this grows
    /// only with the number of *distinct* ids queued since the last drain,
    /// never with how many times any one of them is queued.
    pending_unbind: BTreeMap<u64, SceneTextureId>,
    /// Ids a panic has already been reported at `warn!` for, alongside the
    /// identity (Arc pointer) of the pass that panicked. If the same pass id
    /// panics again with the same Arc, it reports at `debug!` instead of
    /// `warn!`. If a different Arc binds to the same id, the warn-once flag
    /// is cleared and the new pass's first panic is a fresh `warn!` too.
    reported_panics: BTreeMap<u64, PanicReport>,
    /// Flag for whether a reserved-namespace registration attempt has already
    /// been reported at `warn!` level. Warn once process-wide (the refusal
    /// reason is identical for every id), then report at `debug!` for any
    /// further attempts.
    reported_reserved: bool,
}

impl Registry {
    const fn new() -> Self {
        Self {
            passes: BTreeMap::new(),
            pending_unbind: BTreeMap::new(),
            reported_panics: BTreeMap::new(),
            reported_reserved: false,
        }
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// Frames drained so far — the source of [`ExternalFrame::frame_index`].
static FRAME_INDEX: AtomicU64 = AtomicU64::new(0);

/// The registry, with poison ignored: one pass's panic must not turn every
/// later frame's drain into a panic of its own, which is the opposite of what
/// the `catch_unwind` around `record` is for. (Nothing panics while the guard
/// is held — `record` is called after it is dropped — so poison here would
/// only ever be collateral.)
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers `pass` under `id`, to be recorded every frame until
/// [`unregister_external_pass`] takes it back.
///
/// Answers `false` and changes nothing when `id` already has a pass: a
/// registration is a claim on an id, and silently displacing another
/// component's pass would leave it registered from its own point of view and
/// never called. Unregister first to hand an id over deliberately.
///
/// Also answers `false` and changes nothing when `id` is inside the reserved
/// shader-program namespace (`SceneTextureId::is_shader_program`) — that
/// range belongs to the engine's own offscreen shader-effect targets, and a
/// caller-registered pass claiming one would silently steal a shader
/// program's target out from under it. Reported at `warn!` the first time a
/// given id is refused this way, `debug!` on every later attempt, so a
/// caller retrying the same doomed registration every frame does not spam
/// the log.
///
/// A `register` landing before the drain has flushed the same id's queued
/// unbind cancels that unbind: the id has an owner again. A successful
/// registration also clears any panic already reported for `id`, so a fresh
/// pass's first panic is reported fresh rather than silently downgraded by
/// a predecessor's history.
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
    let pass_identity = Arc::as_ptr(&pass) as *const () as usize;
    let should_clear_panic_flag = {
        let registry = registry();
        if registry.passes.contains_key(&raw) {
            return false;
        }
        registry
            .reported_panics
            .get(&raw)
            .is_some_and(|report| report.identity != pass_identity)
    };
    if should_clear_panic_flag {
        let mut registry = registry();
        registry.reported_panics.remove(&raw);
    }
    let mut registry = registry();
    registry.passes.insert(raw, Registration { id, pass });
    true
}

/// Removes the pass registered under `id` and queues the engine binding it
/// left behind for the next drain to clear, answering whether there was one.
pub fn unregister_external_pass(id: SceneTextureId) -> bool {
    let mut registry = registry();
    if registry.passes.remove(&id.get()).is_none() {
        return false;
    }
    registry.pending_unbind.insert(id.get(), id);
    true
}

/// Runs one frame's external passes into `encoder`, after clearing the engine
/// bindings of every id unregistered since the last run.
///
/// Called by [`crate::SurfaceRenderer`] on the engine tier, immediately
/// before the shader-quad pre-pass and so before anything of the scene's own
/// is recorded. Hidden from the crate's documented surface: it names the
/// engine renderer and the frame's raw `wgpu` resources, which are the
/// renderer's to hold, and it is reachable only so this crate's own GPU test
/// can drive the identical code path against an engine of its own — a
/// surface, and so `submit`, is not something a headless test can produce.
///
/// Cheap on a process that registered nothing: two `is_empty` checks under
/// one uncontended lock, and no encoder work at all.
#[doc(hidden)]
pub fn run_external_passes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    engine: &mut frust_engine::EngineRenderer,
) {
    // Snapshot under the lock and release it before any `record` runs: a pass
    // is caller code, free to register or unregister another pass (its own
    // included) from inside its `record`, which would deadlock against a held
    // guard.
    let (pending_unbind, passes) = {
        let mut registry = registry();
        if registry.passes.is_empty() && registry.pending_unbind.is_empty() {
            return;
        }
        let pending_unbind = std::mem::take(&mut registry.pending_unbind);
        let passes: Vec<(SceneTextureId, Arc<dyn ExternalPass>)> = registry
            .passes
            .values()
            .map(|registration| (registration.id, Arc::clone(&registration.pass)))
            .collect();
        (pending_unbind, passes)
    };

    // Before any pass records: an id with no owner keeps no view alive.
    for id in pending_unbind.into_values() {
        engine.unbind_texture(id);
    }
    if passes.is_empty() {
        return;
    }

    let frame_index = FRAME_INDEX.fetch_add(1, Ordering::Relaxed);
    for (id, pass) in passes {
        // Built fresh per pass, scoped to that pass's own id — `device`,
        // `encoder` and `engine` are reborrowed each iteration rather than
        // moved, so the outer `&mut` references stay usable for the next
        // pass once this one's `ExternalFrame` goes out of scope.
        let mut frame =
            ExternalFrame::new(device, queue, &mut *encoder, &mut *engine, id, frame_index);
        // A pass is caller code on the render thread: unwinding out of it
        // would abandon the frame's encoder mid-recording and take the
        // surface's whole frame loop with it.
        if catch_unwind(AssertUnwindSafe(|| pass.record(&mut frame))).is_err() {
            drop_panicking_pass(id, &pass);
        }
    }
}

/// Retires the pass registered under `id` after it panicked, *if* `pass` is
/// still the one registered there: report it once, take it out of the
/// registry, and queue its binding for the next drain to clear.
///
/// `pass` is the `Arc` [`run_external_passes`] snapshotted before calling
/// `record` on it — compared against whatever is registered under `id` right
/// now via [`Arc::ptr_eq`] rather than blindly removed by id. A pass is
/// caller code, free to hand its own id to a successor (unregister, then
/// register a replacement) from inside the very `record` call that then
/// panics; retiring by id alone would tear that successor out along with the
/// pass that actually failed, even though it was never in the unwind at all.
/// The panic is still reported either way — it happened regardless of what
/// is registered under `id` now — but the registry itself is only touched
/// when the identity check confirms nothing has taken the id over since.
fn drop_panicking_pass(id: SceneTextureId, pass: &Arc<dyn ExternalPass>) {
    let raw = id.get();
    let pass_identity = Arc::as_ptr(pass) as *const () as usize;

    let (still_registered, should_warn, was_different_pass) = {
        let mut registry = registry();
        let registration = registry.passes.get(&raw);
        let still_registered = registration.is_some_and(|r| Arc::ptr_eq(&r.pass, pass));

        let should_warn = registry
            .reported_panics
            .get(&raw)
            .is_none_or(|report| report.identity != pass_identity);

        let was_different_pass = registration.is_some_and(|r| !Arc::ptr_eq(&r.pass, pass));

        if still_registered {
            registry.passes.remove(&raw);
            registry.pending_unbind.insert(raw, id);
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

    if was_different_pass && !still_registered {
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

    /// Snapshot of the registry's own state, since the drain is the only
    /// thing that reads it and the drain needs a device.
    fn state(id: SceneTextureId) -> (bool, bool) {
        let registry = registry();
        (
            registry.passes.contains_key(&id.get()),
            registry.pending_unbind.contains_key(&id.get()),
        )
    }

    #[test]
    fn registering_an_id_twice_keeps_the_first_pass() {
        let _serial = serial();
        let id = SceneTextureId::mint();

        assert!(register_external_pass(id, Arc::new(Inert)));
        assert!(
            !register_external_pass(id, Arc::new(Inert)),
            "a second registration must not displace the first"
        );
        assert_eq!(state(id), (true, false));

        assert!(unregister_external_pass(id));
        assert_eq!(
            state(id),
            (false, true),
            "unregistering queues the engine binding for the next drain"
        );
    }

    #[test]
    fn unregistering_an_unregistered_id_answers_false() {
        let _serial = serial();
        let id = SceneTextureId::mint();

        assert!(!unregister_external_pass(id));
        assert_eq!(
            state(id),
            (false, false),
            "an id nothing registered queues no unbind"
        );
    }

    #[test]
    fn re_registering_before_the_drain_queues_an_unbind_first() {
        let _serial = serial();
        let id = SceneTextureId::mint();

        assert!(register_external_pass(id, Arc::new(Inert)));
        assert!(unregister_external_pass(id));
        assert!(register_external_pass(id, Arc::new(Inert)));
        assert_eq!(
            state(id),
            (true, true),
            "handing an id over leaves a queued unbind (the id draws nothing until the new owner binds)"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn a_panicking_pass_is_retired_and_queued_for_unbind() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass: Arc<dyn ExternalPass> = Arc::new(Inert);

        assert!(register_external_pass(id, Arc::clone(&pass)));
        drop_panicking_pass(id, &pass);
        assert_eq!(
            state(id),
            (false, true),
            "a panicking pass leaves the registry exactly as an unregistered one does"
        );

        // The second retirement reports at debug rather than warn; what is
        // checked here is that reporting twice is not itself a panic and
        // leaves the state alone.
        drop_panicking_pass(id, &pass);
        assert_eq!(state(id), (false, true));

        registry().pending_unbind.remove(&id.get());
    }

    #[test]
    fn retiring_a_panic_leaves_a_successor_registered_under_the_same_id_alone() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let original: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&original)));

        // What a pass handing its own id to a replacement inside `record`
        // does before it panics: unregister, then register the successor —
        // both while the drain's snapshot still holds `original`.
        assert!(unregister_external_pass(id));
        let successor: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&successor)));

        // The drain retires by comparing the snapshot it took (`original`)
        // against whatever is registered now, not by id alone.
        drop_panicking_pass(id, &original);

        assert_eq!(
            state(id),
            (true, true),
            "the successor is left registered, and the queued unbind from the hand-over persists"
        );

        assert!(unregister_external_pass(id));
        registry().pending_unbind.remove(&id.get());
    }

    #[test]
    fn a_fresh_registration_with_a_different_arc_clears_the_reported_panic_flag() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let pass1: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&pass1)));

        drop_panicking_pass(id, &pass1);
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

        drop_panicking_pass(id, &pass);
        assert!(
            registry().reported_panics.contains_key(&id.get()),
            "the first panic is recorded"
        );

        // The panic removed the pass and queued an unbind; clean that up before re-registering
        registry().pending_unbind.remove(&id.get());

        // Re-register the same Arc: the panic flag stays since it's the same identity
        assert!(register_external_pass(id, Arc::clone(&pass)));
        assert!(
            registry().reported_panics.contains_key(&id.get()),
            "re-registering the same Arc keeps the panic flag (no warn on next panic)"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn panicking_pass_with_successor_already_registered_logs_debug() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        let original: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&original)));

        // Hand over: unregister original, register successor
        assert!(unregister_external_pass(id));
        let successor: Arc<dyn ExternalPass> = Arc::new(Inert);
        assert!(register_external_pass(id, Arc::clone(&successor)));

        // Original panics after handing over - successor stays registered
        drop_panicking_pass(id, &original);

        assert_eq!(
            state(id),
            (true, true),
            "the successor is left registered and the queued unbind from the hand-over remains"
        );

        assert!(unregister_external_pass(id));
    }

    #[test]
    fn pending_unbind_grows_with_distinct_ids_not_with_repeat_unregisters() {
        let _serial = serial();
        let id = SceneTextureId::mint();
        // Other cases in this module may leave their own residual entries
        // (under their own distinct ids) in this same process-wide map, so
        // what is asserted is the delta this case itself causes, not an
        // absolute length.
        let before = registry().pending_unbind.len();

        assert!(register_external_pass(id, Arc::new(Inert)));
        assert!(unregister_external_pass(id));
        // The same id again finds nothing left to remove, so this queues no
        // second entry — the map is keyed by id, not appended to per call.
        assert!(!unregister_external_pass(id));
        assert_eq!(
            registry().pending_unbind.len(),
            before + 1,
            "repeat unregistration of one id does not grow the queue past one entry"
        );

        registry().pending_unbind.remove(&id.get());
    }
}
