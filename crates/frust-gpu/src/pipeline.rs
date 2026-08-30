//! Render-pipeline variants and the cache that builds them predictably.
//!
//! The philosophy is Impeller's: a frame must never be the first place a
//! pipeline state object gets compiled. Three pieces implement that here.
//!
//! - [`RenderPipelineDesc`] is a **value** describing one pipeline variant —
//!   a [`ShaderId`] plus entry points, vertex layouts, topology, and the four
//!   render-state axes (`blend`, `format`, `sample_count`, `depth`). It holds
//!   no GPU handle, so it can be listed up front, sent to a worker thread, and
//!   hashed.
//! - [`PipelineCache::warm_up`] takes a list of those descs and builds them on
//!   a worker thread when the surface is installed, so the variants an app is
//!   known to need exist before the first frame asks for one. At most one such
//!   worker is ever alive per cache, and [`PipelineCache::shutdown`] — which
//!   `Drop` calls — joins it, so no thread holding a clone of the device
//!   outlives the cache.
//! - [`PipelineCache::get_or_create`] compiles a variant at most once. If the
//!   render thread asks for a variant that is still *queued* for the warm-up
//!   worker, it does not wait its turn: it **steals** the job, builds it
//!   inline, and marks the queue entry done (Impeller's
//!   `PipelineCompileQueue::PerformJobEagerly`). If the worker is already
//!   mid-build on that exact variant, the request waits for it rather than
//!   compiling a second copy.
//!
//! ## The variant key
//!
//! Every desc collapses to a `u64` packed from five axes, the same trick as
//! Impeller's `ContentContextOptions::ToKey`: fixed-width bit fields, one per
//! axis, so lookup is an integer hash and two descs that differ in any axis
//! can never collide.
//!
//! | Bits | Axis |
//! |------|------|
//! | `[0..16)`  | program — shader module + entry points + vertex layouts + topology |
//! | `[16..28)` | color target format |
//! | `[28..40)` | blend state (`None` is its own value) |
//! | `[40..52)` | depth/stencil state (`None` is its own value) |
//! | `[52..56)` | `log2(sample_count)` |
//!
//! wgpu's render-state types are far too wide to pack literally (a
//! `BlendState` alone is six enums), so the three wide axes are **interned**:
//! each distinct value seen gets the next index in a small side table, and the
//! index is what the bit field holds. That is exact — unlike a truncated
//! content hash, no two distinct values can ever share a field — at the cost
//! of a key being meaningful only within one cache instance, which is all it
//! is ever used for.
//!
//! The first axis is the whole *program*, not just the [`ShaderId`], because a
//! variant that reused one module under a different entry point or vertex
//! layout would otherwise share a key with its sibling and silently get the
//! wrong pipeline. Interning the program tuple makes those distinct variants,
//! which is what Impeller's per-pipeline-type maps achieve structurally.
//!
//! A field whose table overflows its width (or a `sample_count` that is not a
//! power of two, which wgpu would reject anyway) yields no key: that request
//! falls back to an uncached compile rather than risking a wrong hit. It takes
//! thousands of distinct blend states to get there, so it is a safety net, not
//! a path.
//!
//! ## The warm-up worker
//!
//! One worker at a time, spawned on demand and **exiting when the queue
//! drains** rather than parking: a surface reinstall calls `warm_up` again,
//! and the choice keeps a long-lived cache from holding an idle thread (and a
//! device clone) for the process lifetime. The liveness flag is set and
//! cleared inside the same critical section that enqueues jobs and claims
//! them, so a worker on its way out can never be mistaken for one that will
//! still pick up the job just queued.
//!
//! A compile that unwinds marks its variant [`JobState::Failed`] instead of
//! leaving it claimable-but-unreachable: the queue entry is already spent, so
//! a `Pending` job nobody re-queues is a job the worker can never build again
//! and `queued_variants` would count forever. `Failed` is terminal for the
//! *queue* only — the next [`PipelineCache::get_or_create`] for that variant
//! rebuilds it, and a second failure unwinds into that caller rather than
//! parking it on a build that will never publish. The worker catches the
//! unwind so one bad variant costs its own pipeline and not the rest of the
//! queue; an inline build does not, because there the requester is the right
//! place for the failure to land.
//!
//! ## Persisted driver cache
//!
//! A [`wgpu::PipelineCache`] — Vulkan-only, seeded from a shell-persisted blob
//! validated by [`crate::pipeline_cache`] — can be handed to
//! [`PipelineCache::new`]. Every pipeline built here is then created against
//! it, so a warm start reuses the driver's previously compiled machine code.
//! On Metal/DX12 the feature does not exist and `None` is passed; nothing else
//! changes.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use crate::shader::{ShaderId, ShaderLibrary};

/// Width of the packed key's program field.
const PROGRAM_BITS: u32 = 16;
/// Width of the packed key's color-format field.
const FORMAT_BITS: u32 = 12;
/// Width of the packed key's blend-state field.
const BLEND_BITS: u32 = 12;
/// Width of the packed key's depth/stencil field.
const DEPTH_BITS: u32 = 12;
/// Width of the packed key's `log2(sample_count)` field.
const SAMPLE_BITS: u32 = 4;

const PROGRAM_SHIFT: u32 = 0;
const FORMAT_SHIFT: u32 = PROGRAM_SHIFT + PROGRAM_BITS;
const BLEND_SHIFT: u32 = FORMAT_SHIFT + FORMAT_BITS;
const DEPTH_SHIFT: u32 = BLEND_SHIFT + BLEND_BITS;
const SAMPLE_SHIFT: u32 = DEPTH_SHIFT + DEPTH_BITS;

/// One vertex buffer's layout, owned so a [`RenderPipelineDesc`] can be stored,
/// cloned, hashed and sent to the warm-up worker.
///
/// The borrowed `wgpu::VertexBufferLayout` this converts to
/// ([`VertexLayout::as_wgpu`]) is built only for the duration of the
/// `create_render_pipeline` call.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct VertexLayout {
    /// Stride in bytes between consecutive elements.
    pub array_stride: wgpu::BufferAddress,
    /// Whether the buffer steps per vertex or per instance.
    pub step_mode: wgpu::VertexStepMode,
    /// The attributes making up one element.
    pub attributes: Vec<wgpu::VertexAttribute>,
}

impl VertexLayout {
    /// A layout stepped per vertex (the common case) over `attributes`.
    #[must_use]
    pub fn per_vertex(
        array_stride: wgpu::BufferAddress,
        attributes: Vec<wgpu::VertexAttribute>,
    ) -> Self {
        Self {
            array_stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes,
        }
    }

    /// The borrowed wgpu view of this layout.
    #[must_use]
    pub fn as_wgpu(&self) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: self.array_stride,
            step_mode: self.step_mode,
            attributes: &self.attributes,
        }
    }
}

/// One render-pipeline variant, as a plain hashable value.
///
/// Build the descs an app needs at start-up, hand them to
/// [`PipelineCache::warm_up`], and keep them around: a per-frame
/// [`PipelineCache::get_or_create`] takes one by reference and never allocates
/// on a hit.
///
/// The pipeline's bind-group layout is always wgpu's **default layout**,
/// deduced from the shader modules — there is no explicit-layout axis in v1,
/// so a variant set that needs one is out of this cache's scope.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderPipelineDesc {
    /// The module holding both entry points, from the cache's own library.
    pub shader: ShaderId,
    /// Vertex entry point name.
    pub vs: Cow<'static, str>,
    /// Fragment entry point name.
    pub fs: Cow<'static, str>,
    /// Vertex buffer layouts, in bind order. Empty for a shader that
    /// generates its own vertices (a fullscreen triangle).
    pub vertex_layouts: Vec<VertexLayout>,
    /// Color blending, or `None` for an opaque write.
    pub blend: Option<wgpu::BlendState>,
    /// Color target format.
    pub format: wgpu::TextureFormat,
    /// MSAA sample count; must be a power of two, `1` for no MSAA.
    pub sample_count: u32,
    /// Depth/stencil state, or `None` for a color-only pass.
    pub depth: Option<wgpu::DepthStencilState>,
    /// Primitive topology. Strip topologies here are non-indexed only: the
    /// desc carries no `strip_index_format`.
    pub topology: wgpu::PrimitiveTopology,
}

impl RenderPipelineDesc {
    /// A minimal opaque variant: no vertex buffers, no blending, no depth,
    /// `sample_count: 1`, triangle list. Set the axes that differ with struct
    /// update syntax.
    #[must_use]
    pub fn new(
        shader: ShaderId,
        vs: impl Into<Cow<'static, str>>,
        fs: impl Into<Cow<'static, str>>,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            shader,
            vs: vs.into(),
            fs: fs.into(),
            vertex_layouts: Vec::new(),
            blend: None,
            format,
            sample_count: 1,
            depth: None,
            topology: wgpu::PrimitiveTopology::TriangleList,
        }
    }
}

/// The part of a desc that identifies the *program*: everything the packed
/// key's first field stands for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProgramKey {
    shader: ShaderId,
    vs: Cow<'static, str>,
    fs: Cow<'static, str>,
    vertex_layouts: Vec<VertexLayout>,
    topology: wgpu::PrimitiveTopology,
}

impl ProgramKey {
    fn of(desc: &RenderPipelineDesc) -> Self {
        Self {
            shader: desc.shader,
            vs: desc.vs.clone(),
            fs: desc.fs.clone(),
            vertex_layouts: desc.vertex_layouts.clone(),
            topology: desc.topology,
        }
    }

    /// Whether `desc` names this same program, without building a `ProgramKey`
    /// (and therefore without allocating) — the per-request hot path.
    fn matches(&self, desc: &RenderPipelineDesc) -> bool {
        self.shader == desc.shader
            && self.vs == desc.vs
            && self.fs == desc.fs
            && self.topology == desc.topology
            && self.vertex_layouts == desc.vertex_layouts
    }
}

/// Assigns each distinct value of one key axis a dense index.
///
/// `None` is returned once the assigned indices would no longer fit the axis's
/// bit field; the caller then treats the whole key as unavailable.
#[derive(Debug)]
struct AxisTable<K> {
    index: HashMap<K, u64>,
    bits: u32,
}

impl<K: std::hash::Hash + Eq + Clone> AxisTable<K> {
    fn new(bits: u32) -> Self {
        Self {
            index: HashMap::new(),
            bits,
        }
    }

    fn intern(&mut self, value: &K) -> Option<u64> {
        if let Some(existing) = self.index.get(value) {
            return Some(*existing);
        }
        let next = self.index.len() as u64;
        if next >= 1 << self.bits {
            return None;
        }
        self.index.insert(value.clone(), next);
        Some(next)
    }
}

/// The program axis's own table: a hash index onto stored [`ProgramKey`]s, so
/// a repeat lookup compares rather than clones.
#[derive(Debug, Default)]
struct ProgramTable {
    /// Program-tuple hash → the indices assigned to programs with that hash.
    /// A chain longer than one entry means a genuine hash collision, which the
    /// stored key comparison then resolves.
    by_hash: HashMap<u64, Vec<u64>>,
    entries: Vec<ProgramKey>,
}

impl ProgramTable {
    fn intern(&mut self, desc: &RenderPipelineDesc) -> Option<u64> {
        let hash = program_hash(desc);
        let chain = self.by_hash.entry(hash).or_default();
        for &candidate in chain.iter() {
            if self.entries[candidate as usize].matches(desc) {
                return Some(candidate);
            }
        }
        let next = self.entries.len() as u64;
        if next >= 1 << PROGRAM_BITS {
            return None;
        }
        chain.push(next);
        self.entries.push(ProgramKey::of(desc));
        Some(next)
    }
}

/// Hashes a desc's program fields without building a [`ProgramKey`].
fn program_hash(desc: &RenderPipelineDesc) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    desc.shader.hash(&mut hasher);
    desc.vs.hash(&mut hasher);
    desc.fs.hash(&mut hasher);
    desc.vertex_layouts.hash(&mut hasher);
    desc.topology.hash(&mut hasher);
    hasher.finish()
}

/// Packs a [`RenderPipelineDesc`] into the `u64` variant key.
#[derive(Debug)]
struct KeyPacker {
    programs: ProgramTable,
    formats: AxisTable<wgpu::TextureFormat>,
    blends: AxisTable<Option<wgpu::BlendState>>,
    depths: AxisTable<Option<wgpu::DepthStencilState>>,
}

impl Default for KeyPacker {
    fn default() -> Self {
        Self {
            programs: ProgramTable::default(),
            formats: AxisTable::new(FORMAT_BITS),
            blends: AxisTable::new(BLEND_BITS),
            depths: AxisTable::new(DEPTH_BITS),
        }
    }
}

impl KeyPacker {
    /// The variant key for `desc`, or `None` if an axis cannot be represented
    /// (an unusable `sample_count`, or an exhausted interning table).
    fn key_for(&mut self, desc: &RenderPipelineDesc) -> Option<u64> {
        // Resolved first so a rejected sample count never leaves a fresh entry
        // behind in a table.
        let sample = sample_field(desc.sample_count)?;
        let program = self.programs.intern(desc)?;
        let format = self.formats.intern(&desc.format)?;
        let blend = self.blends.intern(&desc.blend)?;
        let depth = self.depths.intern(&desc.depth)?;
        Some(
            (program << PROGRAM_SHIFT)
                | (format << FORMAT_SHIFT)
                | (blend << BLEND_SHIFT)
                | (depth << DEPTH_SHIFT)
                | (sample << SAMPLE_SHIFT),
        )
    }
}

/// `log2(sample_count)`, or `None` for a count wgpu could not use anyway.
fn sample_field(sample_count: u32) -> Option<u64> {
    if sample_count == 0 || !sample_count.is_power_of_two() {
        return None;
    }
    let field = u64::from(sample_count.trailing_zeros());
    (field < 1 << SAMPLE_BITS).then_some(field)
}

/// Where a queued warm-up job has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    /// Listed by `warm_up`, nobody has started it.
    Pending,
    /// Some thread is compiling it right now.
    Building,
    /// Built; the pipeline is in `Inner::built`.
    Done,
    /// A compile of this variant unwound. Terminal as far as the warm-up
    /// queue is concerned — its place in the queue is spent, so no worker
    /// will claim it again — but a later request rebuilds it inline.
    Failed,
}

/// One warm-up job: the variant to build and how far along it is.
#[derive(Debug)]
struct Job {
    desc: RenderPipelineDesc,
    state: JobState,
}

/// Everything both the render thread and the warm-up worker touch.
#[derive(Debug)]
struct Inner<P> {
    built: HashMap<u64, Arc<P>>,
    jobs: HashMap<u64, Job>,
    /// Keys in the order `warm_up` listed them. Entries whose job was stolen
    /// or finished are skipped when popped.
    queue: VecDeque<u64>,
    /// How many compiles have actually run — the "compiled once" evidence.
    compiles: u64,
    /// Whether a warm-up worker is draining `queue` right now. Only ever
    /// written while the lock is held, which is what makes "one worker" a
    /// fact rather than a race: see the module docs.
    worker_live: bool,
    /// Set once the cache is shutting down; a worker claims nothing more, so
    /// teardown waits for at most the compile already in flight.
    shutdown: bool,
    /// Bumped every time liveness is claimed, so a departing worker can tell
    /// whether the flag it is about to clear is still its own.
    worker_epoch: u64,
}

impl<P> Default for Inner<P> {
    fn default() -> Self {
        Self {
            built: HashMap::new(),
            jobs: HashMap::new(),
            queue: VecDeque::new(),
            compiles: 0,
            worker_live: false,
            shutdown: false,
            worker_epoch: 0,
        }
    }
}

impl<P> Inner<P> {
    /// Claims the next still-pending job, marking it `Building`.
    ///
    /// Returning `None` also clears `worker_live`, in this same critical
    /// section: a caller that enqueues after this point sees no live worker
    /// and spawns one, so the job it just listed cannot fall between the
    /// worker's last look at the queue and its exit.
    fn claim_next_pending(&mut self) -> Option<(u64, RenderPipelineDesc)> {
        while !self.shutdown
            && let Some(key) = self.queue.pop_front()
        {
            if let Some(job) = self.jobs.get_mut(&key)
                && job.state == JobState::Pending
            {
                job.state = JobState::Building;
                return Some((key, job.desc.clone()));
            }
        }
        self.worker_live = false;
        None
    }
}

/// The shared half of a cache: state plus the condition variable a waiter
/// blocks on while another thread builds the variant it wants.
#[derive(Debug)]
struct Shared<P> {
    inner: Mutex<Inner<P>>,
    built: Condvar,
}

impl<P> Default for Shared<P> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            built: Condvar::new(),
        }
    }
}

/// Locks a mutex, adopting the guard even if a previous holder panicked — this
/// cache never leaves inconsistent state behind, and a poisoned lock must not
/// turn a warm-up hiccup into a crash.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Holds a job `Building` for the duration of one compile, and marks it
/// [`JobState::Failed`] if that compile unwinds — without this a panicking
/// builder would leave every waiter blocked on a build that will never finish.
///
/// `Failed` rather than `Pending` because the job's place in the warm-up queue
/// is already spent: a `Pending` entry no worker can reach is a phantom that
/// `queued_variants` would report for the process lifetime. The failure stays
/// visible to the next requester, which rebuilds the variant inline.
struct BuildGuard<'a, P> {
    shared: &'a Shared<P>,
    key: u64,
    armed: bool,
}

impl<'a, P> BuildGuard<'a, P> {
    fn new(shared: &'a Shared<P>, key: u64) -> Self {
        Self {
            shared,
            key,
            armed: true,
        }
    }

    /// Publishes the finished pipeline, marks the job done, and wakes waiters.
    fn publish(mut self, pipeline: Arc<P>) {
        self.armed = false;
        let mut inner = lock(&self.shared.inner);
        inner.built.insert(self.key, pipeline);
        if let Some(job) = inner.jobs.get_mut(&self.key) {
            job.state = JobState::Done;
        }
        inner.compiles += 1;
        drop(inner);
        self.shared.built.notify_all();
    }
}

/// Clears the worker-liveness flag if a drain leaves by any route other than
/// the ordinary "queue empty" one, which clears it under the lock itself.
///
/// The epoch check is what stops a departing worker from clearing liveness
/// that a *newer* worker has since claimed — without it, an abnormal exit
/// racing a fresh `warm_up` could leave two workers on one queue.
struct LiveWorkerGuard<'a, P> {
    shared: &'a Shared<P>,
    epoch: u64,
}

impl<P> Drop for LiveWorkerGuard<'_, P> {
    fn drop(&mut self) {
        let mut inner = lock(&self.shared.inner);
        if inner.worker_epoch == self.epoch {
            inner.worker_live = false;
        }
    }
}

impl<P> Drop for BuildGuard<'_, P> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut inner = lock(&self.shared.inner);
        if let Some(job) = inner.jobs.get_mut(&self.key) {
            job.state = JobState::Failed;
        }
        drop(inner);
        self.shared.built.notify_all();
    }
}

/// The device-free half of [`PipelineCache`]: key packing, the warm-up queue,
/// eager stealing, and the render-thread mirror.
///
/// Generic over the compiled artifact so all of that policy is exercised by
/// host tests with a fake compile function, with no GPU in the loop — the same
/// split [`crate::caps::TierCaps`] uses to keep tier decisions testable.
#[derive(Debug)]
struct VariantCache<P> {
    shared: Arc<Shared<P>>,
    /// Render-thread mirror of `Shared::built`, so a steady-state hit takes no
    /// lock and can hand out a plain reference.
    local: HashMap<u64, Arc<P>>,
    /// The most recent uncached compile (see the key-exhaustion note on the
    /// module docs). Held only so a reference can be returned.
    overflow: Option<Arc<P>>,
    keys: KeyPacker,
    warned_key_space: bool,
    /// The one warm-up worker, kept so [`VariantCache::shutdown`] can join it.
    worker: Option<std::thread::JoinHandle<()>>,
}

impl<P> Default for VariantCache<P> {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared::default()),
            local: HashMap::new(),
            overflow: None,
            keys: KeyPacker::default(),
            warned_key_space: false,
            worker: None,
        }
    }
}

impl<P> Drop for VariantCache<P> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl<P> VariantCache<P> {
    /// A handle on the shared state, for a warm-up worker.
    fn shared(&self) -> Arc<Shared<P>> {
        Arc::clone(&self.shared)
    }

    /// Returns the variant for `desc`, compiling it at most once.
    fn get_or_create<F>(&mut self, desc: &RenderPipelineDesc, compile: F) -> &P
    where
        F: Fn(&RenderPipelineDesc) -> P,
    {
        let Some(key) = self.keys.key_for(desc) else {
            if !self.warned_key_space {
                self.warned_key_space = true;
                log::error!(
                    "frust-gpu: pipeline variant key space exhausted (or an unusable \
                     sample_count {}); this variant is compiled per request instead of cached",
                    desc.sample_count
                );
            }
            self.overflow = Some(Arc::new(compile(desc)));
            return self
                .overflow
                .as_deref()
                .expect("the uncached variant was just stored");
        };

        if !self.local.contains_key(&key) {
            let built = self.resolve(key, desc, &compile);
            self.local.insert(key, built);
        }
        &self.local[&key]
    }

    /// The shared-state half of `get_or_create`: hit, steal, wait, or build.
    fn resolve<F>(&self, key: u64, desc: &RenderPipelineDesc, compile: &F) -> Arc<P>
    where
        F: Fn(&RenderPipelineDesc) -> P,
    {
        let mut inner = lock(&self.shared.inner);
        loop {
            if let Some(built) = inner.built.get(&key) {
                return Arc::clone(built);
            }
            match inner.jobs.get(&key).map(|job| job.state) {
                // The warm-up worker holds this job: wait for its result
                // rather than compiling a second copy of the same variant.
                Some(JobState::Building) => {
                    inner = self
                        .shared
                        .built
                        .wait(inner)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                // Queued but unstarted — steal it and build it inline. Same
                // for a variant whose last compile unwound: the queue will
                // never come back to it, so the requester rebuilds it here
                // (and eats the failure itself if it unwinds again).
                Some(JobState::Pending | JobState::Failed) => {
                    if let Some(job) = inner.jobs.get_mut(&key) {
                        job.state = JobState::Building;
                    }
                    drop(inner);
                    return self.build(key, desc, compile);
                }
                // Never queued (or queued, finished, and since dropped):
                // record it as in-flight so a concurrent request waits.
                Some(JobState::Done) | None => {
                    inner.jobs.insert(
                        key,
                        Job {
                            desc: desc.clone(),
                            state: JobState::Building,
                        },
                    );
                    drop(inner);
                    return self.build(key, desc, compile);
                }
            }
        }
    }

    /// Runs one compile for an already-claimed (`Building`) job and publishes it.
    fn build<F>(&self, key: u64, desc: &RenderPipelineDesc, compile: &F) -> Arc<P>
    where
        F: Fn(&RenderPipelineDesc) -> P,
    {
        let guard = BuildGuard::new(&self.shared, key);
        let pipeline = Arc::new(compile(desc));
        guard.publish(Arc::clone(&pipeline));
        pipeline
    }

    /// Records `descs` as warm-up jobs, skipping any variant already built or
    /// already queued. Compiles nothing itself.
    ///
    /// Returns whether the caller must start a worker: `true` only when there
    /// is pending work and no live worker to take it, in which case the
    /// liveness flag is claimed here, under the same lock that queued the
    /// jobs. A cache that is shutting down never asks for one.
    fn enqueue(&mut self, descs: &[RenderPipelineDesc]) -> bool {
        let mut inner = lock(&self.shared.inner);
        for desc in descs {
            let Some(key) = self.keys.key_for(desc) else {
                log::error!(
                    "frust-gpu: warm-up variant has no representable key (sample_count {}); \
                     it will be compiled on first use instead",
                    desc.sample_count
                );
                continue;
            };
            if inner.built.contains_key(&key) || inner.jobs.contains_key(&key) {
                continue;
            }
            inner.jobs.insert(
                key,
                Job {
                    desc: desc.clone(),
                    state: JobState::Pending,
                },
            );
            inner.queue.push_back(key);
        }
        if inner.shutdown || inner.worker_live || inner.queue.is_empty() {
            return false;
        }
        inner.worker_live = true;
        inner.worker_epoch += 1;
        true
    }

    /// Builds every still-pending queued job. This is the warm-up worker's
    /// whole body; a job the render thread stole in the meantime is skipped.
    ///
    /// A compile that unwinds is caught here rather than tearing the worker
    /// down with the rest of the queue unbuilt: [`BuildGuard`] has already
    /// marked that one variant failed, and the drain moves on.
    fn drain_queue<F>(shared: &Arc<Shared<P>>, compile: F)
    where
        F: Fn(&RenderPipelineDesc) -> P,
    {
        let _live = LiveWorkerGuard {
            shared,
            epoch: lock(&shared.inner).worker_epoch,
        };
        loop {
            let claimed = lock(&shared.inner).claim_next_pending();
            let Some((key, desc)) = claimed else {
                return;
            };
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let guard = BuildGuard::new(shared, key);
                guard.publish(Arc::new(compile(&desc)));
            }));
            if built.is_err() {
                log::error!(
                    "frust-gpu: a warm-up compile panicked; that variant is left to be \
                     rebuilt on request and the rest of the queue continues"
                );
            }
        }
    }

    /// Queues `descs` and makes sure exactly one worker is draining them.
    ///
    /// `make_compile` is called once per thread that needs one — the worker,
    /// or the calling thread when the spawn fails and the queue has to be
    /// drained inline rather than silently not warmed at all.
    fn warm_up<F>(&mut self, descs: &[RenderPipelineDesc], make_compile: impl Fn() -> F)
    where
        P: Send + Sync + 'static,
        F: Fn(&RenderPipelineDesc) -> P + Send + 'static,
    {
        if !self.enqueue(descs) {
            return;
        }
        // The previous worker has already cleared the liveness flag and is on
        // its way out; joining it here is what keeps "no worker outlives the
        // cache" true of the handle this cache actually holds.
        self.join_worker();

        let worker_shared = self.shared();
        let worker_compile = make_compile();
        let spawned = std::thread::Builder::new()
            .name("frust-gpu pipeline warm-up".to_string())
            .spawn(move || VariantCache::drain_queue(&worker_shared, worker_compile));
        match spawned {
            Ok(handle) => self.worker = Some(handle),
            Err(err) => {
                log::warn!(
                    "frust-gpu: could not spawn the pipeline warm-up thread ({err}); \
                     building the listed variants inline"
                );
                VariantCache::drain_queue(&self.shared(), make_compile());
            }
        }
    }

    /// Stops the warm-up worker and waits for it, so nothing this cache
    /// spawned can still be holding the device once it returns.
    ///
    /// Idempotent, and terminal: the shutdown flag stays set, so a later
    /// `warm_up` queues its variants without starting a worker and they are
    /// built on request instead.
    fn shutdown(&mut self) {
        lock(&self.shared.inner).shutdown = true;
        self.join_worker();
    }

    /// Joins the worker if one is recorded, reporting a panic that escaped it.
    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::error!("frust-gpu: the pipeline warm-up worker panicked");
        }
    }

    /// How many compiles have run since this cache was created.
    fn compiled_variants(&self) -> u64 {
        lock(&self.shared.inner).compiles
    }

    /// How many warm-up jobs are still waiting to be built.
    fn queued_variants(&self) -> usize {
        let inner = lock(&self.shared.inner);
        inner
            .jobs
            .values()
            .filter(|job| job.state == JobState::Pending)
            .count()
    }
}

/// The engine's render-pipeline cache.
///
/// Owns nothing but policy plus the compiled pipelines: the `wgpu::Device` is
/// passed per call, and the shader modules come from the shared
/// [`ShaderLibrary`] handed to [`PipelineCache::new`].
///
/// See the module docs for the variant key, the warm-up queue and the
/// eager-steal rule.
#[derive(Debug)]
pub struct PipelineCache {
    core: VariantCache<wgpu::RenderPipeline>,
    shaders: Arc<ShaderLibrary>,
    driver_cache: Option<wgpu::PipelineCache>,
}

impl PipelineCache {
    /// A cache over `shaders`, optionally seeding every pipeline it builds
    /// from a persisted `driver_cache`.
    ///
    /// `driver_cache` is `None` on every backend but Vulkan; see
    /// [`crate::pipeline_cache`] for how a persisted blob is validated before
    /// it becomes one.
    #[must_use]
    pub fn new(shaders: Arc<ShaderLibrary>, driver_cache: Option<wgpu::PipelineCache>) -> Self {
        Self {
            core: VariantCache::default(),
            shaders,
            driver_cache,
        }
    }

    /// The library this cache resolves [`ShaderId`]s against.
    #[must_use]
    pub fn shaders(&self) -> &ShaderLibrary {
        &self.shaders
    }

    /// The pipeline for `desc`, compiling it if this is the first request.
    ///
    /// A variant queued for the warm-up worker but not yet started is stolen
    /// and built inline; one the worker is already building is waited for.
    /// Either way the variant is compiled exactly once.
    ///
    /// # Panics
    ///
    /// If `desc.shader` was not minted by this cache's [`ShaderLibrary`]. An
    /// id is opaque and library-scoped, so that is a wiring bug, not a runtime
    /// condition.
    pub fn get_or_create(
        &mut self,
        device: &wgpu::Device,
        desc: &RenderPipelineDesc,
    ) -> &wgpu::RenderPipeline {
        let shaders = &self.shaders;
        let driver_cache = self.driver_cache.as_ref();
        self.core.get_or_create(desc, |desc| {
            build_render_pipeline(device, shaders, driver_cache, desc)
        })
    }

    /// Queues `descs` and builds them on a worker thread.
    ///
    /// Call it once the surface (and therefore the device) exists, with every
    /// variant the app is known to draw. Returns immediately, and the render
    /// thread never blocks on the worker — a variant it asks for early is
    /// stolen out of the queue instead.
    ///
    /// Safe to call repeatedly: a surface reinstall enqueues its variants
    /// against the live worker rather than starting a second one, and a call
    /// whose variants are all built already starts nothing at all.
    ///
    /// If the thread cannot be spawned the queue is drained inline instead, so
    /// warm-up degrades to a synchronous build rather than silently not
    /// happening.
    pub fn warm_up(&mut self, device: &wgpu::Device, descs: &[RenderPipelineDesc]) {
        let shaders = Arc::clone(&self.shaders);
        let driver_cache = self.driver_cache.clone();
        self.core
            .warm_up(descs, || compiler(device, &shaders, driver_cache.as_ref()));
    }

    /// Stops warm-up and joins the worker, so no thread holding a clone of the
    /// device outlives this cache.
    ///
    /// [`Drop`] calls this; call it explicitly to order the wait before other
    /// teardown of your own. It is idempotent, and terminal — afterwards
    /// [`Self::warm_up`] records its variants without starting a worker, and
    /// they are compiled on first request.
    pub fn shutdown(&mut self) {
        self.core.shutdown();
    }

    /// How many pipelines this cache has actually compiled.
    #[must_use]
    pub fn compiled_variants(&self) -> u64 {
        self.core.compiled_variants()
    }

    /// How many warm-up variants are still waiting to be built.
    #[must_use]
    pub fn queued_variants(&self) -> usize {
        self.core.queued_variants()
    }
}

/// A self-contained compile function for a worker thread: it owns clones of
/// the device, library and driver cache, all of which are `Arc`-backed
/// handles.
fn compiler(
    device: &wgpu::Device,
    shaders: &Arc<ShaderLibrary>,
    driver_cache: Option<&wgpu::PipelineCache>,
) -> impl Fn(&RenderPipelineDesc) -> wgpu::RenderPipeline + Send + 'static + use<> {
    let device = device.clone();
    let shaders = Arc::clone(shaders);
    let driver_cache = driver_cache.cloned();
    move |desc| build_render_pipeline(&device, &shaders, driver_cache.as_ref(), desc)
}

/// Creates one `wgpu::RenderPipeline` from a desc — the only place in this
/// module that touches the device.
fn build_render_pipeline(
    device: &wgpu::Device,
    shaders: &ShaderLibrary,
    driver_cache: Option<&wgpu::PipelineCache>,
    desc: &RenderPipelineDesc,
) -> wgpu::RenderPipeline {
    let name = shaders.name_of(desc.shader).unwrap_or("<unknown shader>");
    let module = shaders
        .get(desc.shader)
        .expect("a RenderPipelineDesc's ShaderId must come from this cache's ShaderLibrary");
    let layouts: Vec<wgpu::VertexBufferLayout<'_>> = desc
        .vertex_layouts
        .iter()
        .map(VertexLayout::as_wgpu)
        .collect();
    let label = format!(
        "frust-gpu pipeline: {name} [{:?}, msaa x{}]",
        desc.format, desc.sample_count
    );

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&label),
        // No explicit-layout axis in v1 — see `RenderPipelineDesc`'s docs.
        layout: None,
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(desc.vs.as_ref()),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &layouts,
        },
        primitive: wgpu::PrimitiveState {
            topology: desc.topology,
            ..Default::default()
        },
        depth_stencil: desc.depth.clone(),
        multisample: wgpu::MultisampleState {
            count: desc.sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(desc.fs.as_ref()),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: desc.format,
                blend: desc.blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: driver_cache,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Stands in for a `wgpu::RenderPipeline` so the queue/steal policy runs
    /// with no device: it records which variant it was built from.
    #[derive(Debug, PartialEq, Eq)]
    struct FakePipeline {
        sample_count: u32,
        serial: u64,
    }

    /// A compile function that counts its calls, so "compiled once" is an
    /// assertion rather than an inference.
    #[derive(Default)]
    struct Counter(AtomicU64);

    impl Counter {
        fn compile(&self, desc: &RenderPipelineDesc) -> FakePipeline {
            let serial = self.0.fetch_add(1, Ordering::SeqCst);
            FakePipeline {
                sample_count: desc.sample_count,
                serial,
            }
        }

        fn count(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn desc() -> RenderPipelineDesc {
        RenderPipelineDesc::new(
            ShaderId::from_raw(0),
            "vs_main",
            "fs_main",
            wgpu::TextureFormat::Rgba8Unorm,
        )
    }

    #[test]
    fn a_repeated_request_compiles_once() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Counter::default();
        let desc = desc();

        let first = cache.get_or_create(&desc, |d| counter.compile(d)).serial;
        for _ in 0..8 {
            let again = cache.get_or_create(&desc, |d| counter.compile(d)).serial;
            assert_eq!(again, first, "every repeat must return the same pipeline");
        }
        assert_eq!(counter.count(), 1);
        assert_eq!(cache.compiled_variants(), 1);
    }

    #[test]
    fn each_key_axis_is_its_own_variant() {
        let base = desc();
        let variants = [
            RenderPipelineDesc {
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                ..base.clone()
            },
            RenderPipelineDesc {
                format: wgpu::TextureFormat::Bgra8Unorm,
                ..base.clone()
            },
            RenderPipelineDesc {
                sample_count: 4,
                ..base.clone()
            },
            RenderPipelineDesc {
                depth: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                ..base.clone()
            },
            RenderPipelineDesc {
                shader: ShaderId::from_raw(1),
                ..base.clone()
            },
            // Same shader id, different entry point: the program axis, not a
            // silent collision with `base`.
            RenderPipelineDesc {
                fs: "fs_other".into(),
                ..base.clone()
            },
            // Same shader id and entry points, different vertex layout.
            RenderPipelineDesc {
                vertex_layouts: vec![VertexLayout::per_vertex(
                    16,
                    vec![wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    }],
                )],
                ..base.clone()
            },
            RenderPipelineDesc {
                topology: wgpu::PrimitiveTopology::LineList,
                ..base.clone()
            },
        ];

        let mut packer = KeyPacker::default();
        let base_key = packer.key_for(&base).expect("base key");
        let mut seen = vec![base_key];
        for variant in &variants {
            let key = packer.key_for(variant).expect("variant key");
            assert!(
                !seen.contains(&key),
                "{variant:?} must not reuse an existing key"
            );
            seen.push(key);
        }

        // And each of them really is a separate compile.
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Counter::default();
        cache.get_or_create(&base, |d| counter.compile(d));
        for variant in &variants {
            cache.get_or_create(variant, |d| counter.compile(d));
        }
        assert_eq!(counter.count(), 1 + variants.len() as u64);
    }

    #[test]
    fn a_key_is_stable_across_repeated_packing() {
        let mut packer = KeyPacker::default();
        let desc = desc();
        let first = packer.key_for(&desc).expect("key");
        for _ in 0..4 {
            assert_eq!(packer.key_for(&desc), Some(first));
        }
    }

    #[test]
    fn an_unusable_sample_count_has_no_key_and_is_not_cached() {
        let mut packer = KeyPacker::default();
        assert_eq!(
            packer.key_for(&RenderPipelineDesc {
                sample_count: 3,
                ..desc()
            }),
            None
        );
        assert_eq!(
            packer.key_for(&RenderPipelineDesc {
                sample_count: 0,
                ..desc()
            }),
            None
        );

        // The uncached fallback still returns a usable pipeline, it just
        // recompiles per request.
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Counter::default();
        let odd = RenderPipelineDesc {
            sample_count: 3,
            ..desc()
        };
        assert_eq!(
            cache
                .get_or_create(&odd, |d| counter.compile(d))
                .sample_count,
            3
        );
        cache.get_or_create(&odd, |d| counter.compile(d));
        assert_eq!(counter.count(), 2, "an unkeyable variant is never cached");
    }

    #[test]
    fn draining_the_queue_builds_every_listed_variant_once() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Counter::default();
        let listed: Vec<_> = [1u32, 2, 4]
            .into_iter()
            .map(|sample_count| RenderPipelineDesc {
                sample_count,
                ..desc()
            })
            .collect();

        cache.enqueue(&listed);
        assert_eq!(cache.queued_variants(), 3);
        VariantCache::drain_queue(&cache.shared(), |d| counter.compile(d));
        assert_eq!(counter.count(), 3);
        assert_eq!(cache.queued_variants(), 0);

        // Every warmed variant is now a hit, not a compile.
        for desc in &listed {
            cache.get_or_create(desc, |d| counter.compile(d));
        }
        assert_eq!(counter.count(), 3, "a warmed variant must not recompile");
    }

    #[test]
    fn enqueueing_the_same_variant_twice_queues_one_job() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let listed = [desc(), desc()];
        cache.enqueue(&listed);
        cache.enqueue(&listed);
        assert_eq!(cache.queued_variants(), 1);
    }

    #[test]
    fn a_render_thread_request_steals_a_queued_job_and_the_queue_skips_it() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Counter::default();
        let listed: Vec<_> = [1u32, 2, 4]
            .into_iter()
            .map(|sample_count| RenderPipelineDesc {
                sample_count,
                ..desc()
            })
            .collect();
        cache.enqueue(&listed);

        // The render thread wants the last-listed variant before the worker
        // has started: it builds it inline rather than waiting its turn.
        let stolen = cache
            .get_or_create(&listed[2], |d| counter.compile(d))
            .serial;
        assert_eq!(stolen, 0, "the steal is the first compile to run");
        assert_eq!(counter.count(), 1);
        assert_eq!(cache.queued_variants(), 2, "the stolen job is done");

        // The worker then builds only what is left — the stolen entry is
        // marked done, not rebuilt.
        VariantCache::drain_queue(&cache.shared(), |d| counter.compile(d));
        assert_eq!(
            counter.count(),
            3,
            "the stolen variant must not compile twice"
        );
        assert_eq!(
            cache
                .get_or_create(&listed[2], |d| counter.compile(d))
                .serial,
            stolen
        );
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn a_worker_thread_drain_publishes_to_the_render_thread() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let listed: Vec<_> = (0..6)
            .map(|i| RenderPipelineDesc {
                shader: ShaderId::from_raw(i),
                ..desc()
            })
            .collect();
        cache.enqueue(&listed);

        let shared = cache.shared();
        let worker_counter = Arc::new(Counter::default());
        let thread_counter = Arc::clone(&worker_counter);
        let worker = std::thread::spawn(move || {
            VariantCache::drain_queue(&shared, move |d| thread_counter.compile(d));
        });
        worker.join().expect("the warm-up worker must not panic");

        assert_eq!(worker_counter.count(), 6);
        assert_eq!(cache.compiled_variants(), 6);

        // The render thread now sees the worker's pipelines through the
        // shared map; its own compile function is never called.
        let render_counter = Counter::default();
        for desc in &listed {
            cache.get_or_create(desc, |d| render_counter.compile(d));
        }
        assert_eq!(render_counter.count(), 0);
    }

    #[test]
    fn an_abandoned_build_marks_the_job_failed_rather_than_pending() {
        let cache = VariantCache::<FakePipeline>::default();
        let shared = cache.shared();
        let guard = BuildGuard::new(&shared, 42);
        lock(&shared.inner).jobs.insert(
            42,
            Job {
                desc: desc(),
                state: JobState::Building,
            },
        );
        // Dropping without publishing is what an unwinding compile does.
        drop(guard);
        assert_eq!(
            lock(&shared.inner).jobs.get(&42).map(|job| job.state),
            Some(JobState::Failed),
            "an abandoned build must not leave a Pending job no worker can claim"
        );
    }

    /// A compile that panics once, then behaves — the failure mode a driver
    /// or a malformed shader produces on exactly one variant.
    struct PanicOnce {
        counter: Arc<Counter>,
        armed: AtomicBool,
    }

    impl PanicOnce {
        fn new(counter: &Arc<Counter>) -> Self {
            Self {
                counter: Arc::clone(counter),
                armed: AtomicBool::new(true),
            }
        }

        fn compile(&self, desc: &RenderPipelineDesc) -> FakePipeline {
            assert!(
                !self.armed.swap(false, Ordering::SeqCst),
                "the fake compile fails its first call"
            );
            self.counter.compile(desc)
        }
    }

    #[test]
    fn a_panicking_warm_up_compile_costs_one_variant_and_not_the_queue() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Arc::new(Counter::default());
        let failing = PanicOnce::new(&counter);
        let listed: Vec<_> = [1u32, 2, 4]
            .into_iter()
            .map(|sample_count| RenderPipelineDesc {
                sample_count,
                ..desc()
            })
            .collect();

        cache.enqueue(&listed);
        VariantCache::drain_queue(&cache.shared(), |d| failing.compile(d));

        // The unwind took its own variant down and nothing else: the drain
        // carried on and built the other two.
        assert_eq!(counter.count(), 2);
        assert_eq!(
            cache.queued_variants(),
            0,
            "a failed job must not be counted as still queued forever"
        );

        // And the failed variant is rebuilt on request rather than waited on.
        assert_eq!(
            cache
                .get_or_create(&listed[0], |d| counter.compile(d))
                .sample_count,
            1
        );
        assert_eq!(counter.count(), 3);

        // Everything the worker did build is still a hit.
        for desc in &listed {
            cache.get_or_create(desc, |d| counter.compile(d));
        }
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn a_panicking_inline_build_surfaces_to_the_requester_and_leaves_it_rebuildable() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Arc::new(Counter::default());
        let failing = PanicOnce::new(&counter);
        let desc = desc();
        cache.enqueue(std::slice::from_ref(&desc));

        // The render thread steals the queued job; its compile unwinds, and
        // the failure lands on the requester rather than anywhere silent.
        let stolen = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.get_or_create(&desc, |d| failing.compile(d));
        }));
        assert!(
            stolen.is_err(),
            "an inline compile must not swallow a panic"
        );
        assert_eq!(counter.count(), 0);

        // The next request rebuilds it instead of blocking on a build that
        // will never publish.
        assert_eq!(cache.get_or_create(&desc, |d| counter.compile(d)).serial, 0);
        assert_eq!(counter.count(), 1);
        assert_eq!(cache.queued_variants(), 0);
    }

    /// A latch a fake compile parks on, so a warm-up worker can be held
    /// mid-drain while the test makes further `warm_up` calls against the
    /// same cache.
    #[derive(Default)]
    struct Latch {
        open: Mutex<bool>,
        changed: Condvar,
    }

    impl Latch {
        fn wait(&self) {
            let mut open = lock(&self.open);
            while !*open {
                open = self
                    .changed
                    .wait(open)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }

        fn open(&self) {
            *lock(&self.open) = true;
            self.changed.notify_all();
        }
    }

    fn shader_variants(count: u32) -> Vec<RenderPipelineDesc> {
        (0..count)
            .map(|i| RenderPipelineDesc {
                shader: ShaderId::from_raw(i),
                ..desc()
            })
            .collect()
    }

    #[test]
    fn repeated_warm_up_calls_keep_at_most_one_worker() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Arc::new(Counter::default());
        let latch = Arc::new(Latch::default());
        let workers = AtomicU64::new(0);
        let listed = shader_variants(6);

        // Every worker this cache starts calls the factory exactly once, so
        // counting factory calls counts workers.
        let make_compile = || {
            workers.fetch_add(1, Ordering::SeqCst);
            let counter = Arc::clone(&counter);
            let latch = Arc::clone(&latch);
            let held = AtomicBool::new(false);
            move |d: &RenderPipelineDesc| {
                if !held.swap(true, Ordering::SeqCst) {
                    latch.wait();
                }
                counter.compile(d)
            }
        };

        // One worker starts on the first listed variant and is held there,
        // so it is unambiguously still live for every later call.
        cache.warm_up(&listed[..1], make_compile);
        for _ in 0..8 {
            cache.warm_up(&listed, make_compile);
        }
        assert_eq!(
            workers.load(Ordering::SeqCst),
            1,
            "a live worker must absorb further warm-up calls, not be joined by more"
        );

        latch.open();
        cache.shutdown();

        // Whatever the worker did not reach before shutdown is built on
        // request; either way each variant is compiled exactly once.
        for desc in &listed {
            cache.get_or_create(desc, |d| counter.compile(d));
        }
        assert_eq!(counter.count(), listed.len() as u64);
        assert_eq!(cache.compiled_variants(), listed.len() as u64);
    }

    #[test]
    fn dropping_the_cache_joins_its_worker() {
        // Stands in for the `wgpu::Device` handle a real compile function
        // clones into the worker: if the thread outlived the cache, this
        // clone would still be alive after the drop.
        let device = Arc::new(());
        let counter = Arc::new(Counter::default());
        let listed = shader_variants(4);

        let mut cache = VariantCache::<FakePipeline>::default();
        cache.warm_up(&listed, || {
            let device = Arc::clone(&device);
            let counter = Arc::clone(&counter);
            move |d: &RenderPipelineDesc| {
                let _held = Arc::clone(&device);
                counter.compile(d)
            }
        });
        drop(cache);

        assert_eq!(
            Arc::strong_count(&device),
            1,
            "no warm-up worker may outlive the cache that started it"
        );
    }

    #[test]
    fn warming_up_after_shutdown_starts_no_worker_and_still_builds_on_request() {
        let mut cache = VariantCache::<FakePipeline>::default();
        let counter = Arc::new(Counter::default());
        let workers = AtomicU64::new(0);
        let listed = shader_variants(3);

        cache.shutdown();
        // Idempotent: a second call is a no-op, not a double join.
        cache.shutdown();
        cache.warm_up(&listed, || {
            workers.fetch_add(1, Ordering::SeqCst);
            let counter = Arc::clone(&counter);
            move |d: &RenderPipelineDesc| counter.compile(d)
        });

        assert_eq!(workers.load(Ordering::SeqCst), 0);
        for desc in &listed {
            cache.get_or_create(desc, |d| counter.compile(d));
        }
        assert_eq!(counter.count(), listed.len() as u64);
    }

    /// Pops a validation error scope, pumping the device until the pop
    /// resolves — the same drain `frust-render`'s shader-effect compile uses,
    /// since the pop is a future a plain block-on would park on.
    fn drain_error_scope(
        device: &wgpu::Device,
        scope: wgpu::ErrorScopeGuard,
    ) -> Option<wgpu::Error> {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(scope.pop());
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(error) => return error,
                Poll::Pending => {
                    let _ = device.poll(wgpu::PollType::wait_indefinitely());
                }
            }
        }
    }

    /// One real pipeline on real hardware: the host tests above all run
    /// against a fake compile function, so this is the only case that proves
    /// a `RenderPipelineDesc` actually describes a pipeline wgpu accepts —
    /// entry points, vertex layout, color target and multisample state
    /// included — and that the cache still compiles it exactly once.
    #[test]
    #[ignore = "requires a GPU (Metal/Vulkan); run locally with `cargo test -p frust-gpu -- --ignored`"]
    fn creates_one_real_pipeline() {
        const WGSL: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
"#;

        let (device, _queue) = pollster::block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            // The environment-aware initializer, so `WGPU_ADAPTER_NAME` picks
            // the GPU on a multi-adapter host instead of the run silently
            // landing on whichever one enumerates first.
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            println!("frust-gpu pipeline test adapter: {:?}", adapter.get_info());
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-gpu pipeline test device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device")
        });

        let mut library = ShaderLibrary::new();
        let shader = library.insert_wgsl(&device, "fullscreen-green", WGSL);
        let mut cache = PipelineCache::new(Arc::new(library), None);

        let desc = RenderPipelineDesc::new(
            shader,
            "vs_main",
            "fs_main",
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        for _ in 0..2 {
            let _pipeline = cache.get_or_create(&device, &desc);
        }
        let error = drain_error_scope(&device, scope);

        assert!(error.is_none(), "pipeline creation raised {error:?}");
        assert_eq!(
            cache.compiled_variants(),
            1,
            "a repeat request must reuse the pipeline"
        );
    }
}
