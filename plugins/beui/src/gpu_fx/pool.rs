//! Offscreen render targets for the 3D substrate, pooled per component and
//! reaped by age, plus the bookkeeping that keeps one `SceneTextureId`
//! binding in step with whatever target is currently live under it.
//!
//! # Why a pool at all
//!
//! A 3D component re-renders its faces into an offscreen texture every frame
//! the surface presents. Allocating that texture per frame would mint a fresh
//! `wgpu::Texture`, view and depth attachment on every vsync; allocating it
//! per *size* and never releasing it would leak every size a component has
//! ever animated through. This pool is the middle: a target is keyed, reused
//! while it keeps being asked for, and dropped once it has gone unasked-for
//! long enough to be genuinely gone rather than merely paused.
//!
//! The policy shape is deliberately the engine's own, not a second invention
//! — `frust_gpu::effects` (the shader-quad substrate) reached the same two
//! problems first and its answers are reused here by value:
//!
//! - **Quantization.** A key's extent is rounded up to the next
//!   [`TARGET_QUANTUM`] before it becomes a key, so a component animating its
//!   size pixel by pixel (a spring settling, a drag) reuses one texture
//!   across a whole band instead of minting one per frame. The renderer still
//!   draws into — and the engine is still handed — the component's *exact*
//!   requested extent, confined to that sub-rect of the (possibly larger)
//!   texture by a viewport; the allocated size never reaches the display
//!   list. The cost is over-allocation for a small target: a 48x48 face still
//!   occupies a [`TARGET_QUANTUM`]-square texture. That is the same trade the
//!   engine makes, and reuse across an animation is worth more than the bytes.
//! - **Age-based reap.** A key absent from [`MAX_UNSEEN_FRAMES`] consecutive
//!   frames' live set is dropped ([`TargetPool::reap`]). A size a still-live
//!   component has resized away from ages out on its own; a component that
//!   simply stopped painting for a moment finds its target still parked.
//!
//! # Binding lifetime
//!
//! [`Binding`] is the other half: the engine composites whatever view is
//! registered under the pass's `SceneTextureId`, and that registration has to
//! follow the pooled target it names. Extent alone is not enough to decide
//! whether a re-bind is owed — a reap can drop and recreate a target at the
//! identical extent between two frames, leaving the engine sampling a texture
//! nothing writes any more. Every target therefore carries a
//! [`FxTarget::generation`], and [`Binding::needs_rebind`] compares the pair.
//!
//! This module performs no binding itself: it decides, and
//! [`crate::gpu_fx::schedule`] calls `ExternalFrame::bind_texture` /
//! `unbind_texture` with the answer. That split is what keeps every rule here
//! testable without a GPU.

use std::collections::HashMap;

use frust::gpu::wgpu;
use std::sync::atomic::{AtomicU64, Ordering};

use super::quad3d::{COLOR_FORMAT, DEPTH_FORMAT};

/// Extent quantum a target's key is rounded up to, mirroring
/// `frust_gpu::effects::quantized_target_key`'s own 256px quantum (which is
/// itself `frust_gpu::pool::TexturePool`'s). Chosen there, and kept here, so
/// a resize crossing one texel does not mint a texture.
pub const TARGET_QUANTUM: u32 = 256;

/// How many consecutive frames a [`TargetKey`] may go unasked-for before
/// [`TargetPool::reap`] drops its GPU resources.
///
/// 120 frames — the same window `frust_gpu::effects::MAX_UNSEEN_FRAMES` uses
/// for a whole shader program, and for the same reason: long enough that a
/// component paused mid-animation (or carried through a brief scene-diff
/// hiccup) is never mistaken for gone, short enough that navigating away from
/// a screen full of 3D cards reclaims their targets within a couple of
/// seconds rather than holding them for the session.
pub const MAX_UNSEEN_FRAMES: u64 = 120;

/// Largest side, in texels, this pool will allocate. A request past it is
/// clamped rather than refused: an oversized target is a component asking for
/// more than any adapter guarantees, and drawing it slightly softer beats
/// drawing nothing.
pub const MAX_TARGET_SIDE: u32 = 4096;

/// Process-wide counter behind [`FxComponentId::mint`].
static NEXT_COMPONENT_ID: AtomicU64 = AtomicU64::new(1);

/// Identifies one 3D component instance across frames — the first half of a
/// [`TargetKey`].
///
/// Minted once when the component acquires its GPU handle and held for that
/// component's whole life, so two instances of the same widget type never
/// share a target and one instance keeps its target across a resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FxComponentId(u64);

impl FxComponentId {
    /// Mints a fresh, process-unique id.
    #[must_use]
    pub fn mint() -> Self {
        Self(NEXT_COMPONENT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// The opaque value this id wraps, for logging and debug labels.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Rounds `extent` up to the next [`TARGET_QUANTUM`] multiple, clamped into
/// `1..=`[`MAX_TARGET_SIDE`].
///
/// A zero request answers the quantum rather than zero: a zero-sized texture
/// is not creatable, and a caller asking for one has already been rejected
/// upstream — this only keeps the arithmetic total.
#[must_use]
pub const fn quantize(extent: u32) -> u32 {
    let clamped = if extent > MAX_TARGET_SIDE {
        MAX_TARGET_SIDE
    } else if extent == 0 {
        1
    } else {
        extent
    };
    let quanta = clamped.div_ceil(TARGET_QUANTUM);
    let quantized = quanta * TARGET_QUANTUM;
    if quantized > MAX_TARGET_SIDE {
        MAX_TARGET_SIDE
    } else {
        quantized
    }
}

/// Clamps a *requested* extent into what a target can actually hold, without
/// quantizing it — the extent the renderer's viewport and the engine's
/// binding both use.
#[must_use]
pub const fn clamp_requested(extent: u32) -> u32 {
    if extent == 0 {
        1
    } else if extent > MAX_TARGET_SIDE {
        MAX_TARGET_SIDE
    } else {
        extent
    }
}

/// What one pooled target is keyed by: the component that owns it and the
/// quantized extent it was allocated at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetKey {
    component: FxComponentId,
    width: u32,
    height: u32,
}

impl TargetKey {
    /// The key a component's *requested* extent resolves to, quantized.
    #[must_use]
    pub const fn new(
        component: FxComponentId,
        requested_width: u32,
        requested_height: u32,
    ) -> Self {
        Self {
            component,
            width: quantize(requested_width),
            height: quantize(requested_height),
        }
    }

    /// The component half of this key.
    #[must_use]
    pub const fn component(self) -> FxComponentId {
        self.component
    }

    /// The quantized extent this key allocates at — never the requested one.
    #[must_use]
    pub const fn allocated(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// One pooled offscreen target: a colour attachment the 3D pass renders into,
/// an optional depth attachment for multi-quad occlusion, and the generation
/// that tells a stale binding apart from a live one.
pub struct FxTarget {
    color: wgpu::TextureView,
    depth: Option<wgpu::TextureView>,
    allocated: (u32, u32),
    generation: u64,
}

impl FxTarget {
    /// The colour view the 3D pass renders into and the engine composites.
    #[must_use]
    pub const fn color(&self) -> &wgpu::TextureView {
        &self.color
    }

    /// The depth view, present only for a target allocated with depth.
    #[must_use]
    pub fn depth(&self) -> Option<&wgpu::TextureView> {
        self.depth.as_ref()
    }

    /// The quantized extent this target was allocated at.
    #[must_use]
    pub const fn allocated(&self) -> (u32, u32) {
        self.allocated
    }

    /// Increments once per *allocation*, so an extent that is unchanged across
    /// a reap-and-recreate is still recognisably a different texture. See the
    /// module docs' *Binding lifetime*.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

/// A pooled target plus the frame it was last asked for.
struct Entry {
    target: FxTarget,
    last_seen: u64,
    has_depth: bool,
}

/// Whether an entry last asked for at `last_seen` has aged out by frame `now`.
///
/// A frame counter only ever advances, but a pool that has never seen a frame
/// sits at `now == 0`; the subtraction is saturating so neither case can
/// reap something younger than the window.
#[must_use]
pub const fn is_stale(last_seen: u64, now: u64) -> bool {
    now.saturating_sub(last_seen) > MAX_UNSEEN_FRAMES
}

/// The per-component target pool.
///
/// Not `Sync` on its own account — it lives inside the pass's mutex (see
/// [`crate::gpu_fx::schedule`]), which is what serialises the concurrent
/// `record` two engine-tier surfaces can produce.
#[derive(Default)]
pub struct TargetPool {
    entries: HashMap<TargetKey, Entry>,
    frame: u64,
    next_generation: u64,
}

impl TargetPool {
    /// An empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a frame at `frame_index` — the counter every later
    /// [`Self::acquire`] marks its key seen at, and the one [`Self::reap`]
    /// measures age against.
    ///
    /// Monotone by clamping rather than by trust: `ExternalFrame::frame_index`
    /// is process-wide, so a pool created mid-session starts behind it, and a
    /// pool must never be walked backwards by a counter that somehow did.
    pub fn begin_frame(&mut self, frame_index: u64) {
        self.frame = self.frame.max(frame_index);
    }

    /// The target for `key`, creating it if this is a size the component has
    /// not asked for lately, and marking it seen this frame either way.
    ///
    /// Answers `None` only when the device refuses the allocation, which the
    /// caller treats as "draw nothing this frame" rather than as an error.
    /// An existing entry allocated *without* depth is replaced when depth is
    /// now wanted (and kept when it is not — a target with a depth attachment
    /// serves a depth-free frame perfectly well).
    pub fn acquire(
        &mut self,
        device: &wgpu::Device,
        key: TargetKey,
        want_depth: bool,
        label: &str,
    ) -> Option<&FxTarget> {
        let frame = self.frame;
        let reuse = self
            .entries
            .get(&key)
            .is_some_and(|entry| entry.has_depth || !want_depth);
        if !reuse {
            let generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1);
            let target = create_target(device, key, want_depth, generation, label)?;
            self.entries.insert(
                key,
                Entry {
                    target,
                    last_seen: frame,
                    has_depth: want_depth,
                },
            );
        }
        let entry = self.entries.get_mut(&key)?;
        entry.last_seen = frame;
        Some(&entry.target)
    }

    /// Drops every key that has gone unasked-for for more than
    /// [`MAX_UNSEEN_FRAMES`], answering the keys it dropped.
    ///
    /// The answer is what lets the caller notice that the key currently bound
    /// under the pass's `SceneTextureId` is gone and unbind it, rather than
    /// leaving the engine sampling a texture nothing writes.
    pub fn reap(&mut self) -> Vec<TargetKey> {
        let now = self.frame;
        let reaped: Vec<TargetKey> = self
            .entries
            .iter()
            .filter(|(_, entry)| is_stale(entry.last_seen, now))
            .map(|(key, _)| *key)
            .collect();
        for key in &reaped {
            self.entries.remove(key);
        }
        reaped
    }

    /// How many targets are resident.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pool holds no target at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The frame this pool last opened.
    #[must_use]
    pub const fn frame(&self) -> u64 {
        self.frame
    }
}

/// Allocates one target's colour texture (and, when asked, its depth
/// attachment) at `key`'s quantized extent.
fn create_target(
    device: &wgpu::Device,
    key: TargetKey,
    want_depth: bool,
    generation: u64,
    label: &str,
) -> Option<FxTarget> {
    let (width, height) = key.allocated();
    if width == 0 || height == 0 {
        return None;
    }
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let color = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("frust-beui gpu_fx colour: {label}")),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let depth = want_depth.then(|| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("frust-beui gpu_fx depth: {label}")),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    });
    Some(FxTarget {
        color,
        depth,
        allocated: (width, height),
        generation,
    })
}

/// What is currently registered under the pass's `SceneTextureId`, and the
/// rule for when that registration is owed a refresh.
///
/// Holds no `wgpu` handle: the engine owns the view once it is bound, and
/// this only remembers enough to answer [`Self::needs_rebind`] honestly.
#[derive(Default)]
pub struct Binding {
    bound: Option<BoundTarget>,
}

/// The extent and generation the live registration was made for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct BoundTarget {
    extent: (u32, u32),
    generation: u64,
}

impl Binding {
    /// Nothing bound yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a bind is owed for a target at `extent` with `generation`.
    ///
    /// True when nothing is bound, when the requested extent changed (the
    /// engine maps the destination onto exactly these texels, so an extent
    /// change is a mapping change), **or** when the generation changed behind
    /// an unchanged extent — a reaped-and-recreated target, which comparing
    /// extent alone would miss, leaving the engine sampling a texture nothing
    /// writes any more.
    #[must_use]
    pub fn needs_rebind(&self, extent: (u32, u32), generation: u64) -> bool {
        self.bound != Some(BoundTarget { extent, generation })
    }

    /// Records that a bind for `extent`/`generation` has just been made.
    pub fn record_bind(&mut self, extent: (u32, u32), generation: u64) {
        self.bound = Some(BoundTarget { extent, generation });
    }

    /// Forgets the live registration, answering whether there was one — the
    /// caller unbinds exactly when this says `true`, and the next
    /// [`Self::needs_rebind`] is `true` again even for the identical target.
    pub fn take(&mut self) -> bool {
        self.bound.take().is_some()
    }

    /// Whether anything is registered right now.
    #[must_use]
    pub const fn is_bound(&self) -> bool {
        self.bound.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Binding, FxComponentId, MAX_TARGET_SIDE, MAX_UNSEEN_FRAMES, TARGET_QUANTUM, TargetKey,
        TargetPool, clamp_requested, is_stale, quantize,
    };

    #[test]
    fn a_component_id_is_process_unique() {
        let a = FxComponentId::mint();
        let b = FxComponentId::mint();
        assert_ne!(a, b);
        assert_ne!(a.get(), b.get());
    }

    #[test]
    fn quantization_rounds_up_to_the_quantum() {
        assert_eq!(quantize(1), TARGET_QUANTUM);
        assert_eq!(quantize(TARGET_QUANTUM), TARGET_QUANTUM);
        assert_eq!(quantize(TARGET_QUANTUM + 1), TARGET_QUANTUM * 2);
        assert_eq!(quantize(TARGET_QUANTUM * 2), TARGET_QUANTUM * 2);
    }

    #[test]
    fn quantization_never_answers_zero_and_never_exceeds_the_ceiling() {
        assert_eq!(quantize(0), TARGET_QUANTUM);
        assert_eq!(quantize(u32::MAX), MAX_TARGET_SIDE);
        assert_eq!(quantize(MAX_TARGET_SIDE + 1), MAX_TARGET_SIDE);
    }

    #[test]
    fn a_requested_extent_is_clamped_but_not_quantized() {
        assert_eq!(clamp_requested(0), 1);
        assert_eq!(clamp_requested(37), 37);
        assert_eq!(clamp_requested(u32::MAX), MAX_TARGET_SIDE);
    }

    /// The whole point of quantization: a component resizing pixel by pixel
    /// resolves to one key across a band, so nothing is reallocated.
    #[test]
    fn nearby_requests_share_one_key() {
        let component = FxComponentId::mint();
        let a = TargetKey::new(component, 300, 180);
        let b = TargetKey::new(component, 301, 181);
        assert_eq!(a, b);
        assert_eq!(a.allocated(), (TARGET_QUANTUM * 2, TARGET_QUANTUM));
    }

    #[test]
    fn two_components_never_share_a_key_at_the_same_size() {
        let a = TargetKey::new(FxComponentId::mint(), 128, 128);
        let b = TargetKey::new(FxComponentId::mint(), 128, 128);
        assert_ne!(a, b);
    }

    #[test]
    fn staleness_is_measured_against_the_unseen_window() {
        assert!(!is_stale(0, MAX_UNSEEN_FRAMES));
        assert!(is_stale(0, MAX_UNSEEN_FRAMES + 1));
        assert!(!is_stale(10, 5), "a pool never reaps into the future");
    }

    #[test]
    fn a_frame_counter_never_walks_backwards() {
        let mut pool = TargetPool::new();
        pool.begin_frame(40);
        pool.begin_frame(7);
        assert_eq!(pool.frame(), 40);
    }

    #[test]
    fn an_empty_pool_reaps_nothing() {
        let mut pool = TargetPool::new();
        pool.begin_frame(MAX_UNSEEN_FRAMES * 4);
        assert!(pool.reap().is_empty());
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn nothing_is_bound_to_begin_with() {
        let binding = Binding::new();
        assert!(!binding.is_bound());
        assert!(binding.needs_rebind((64, 64), 0));
    }

    #[test]
    fn a_recorded_bind_is_not_owed_a_refresh() {
        let mut binding = Binding::new();
        binding.record_bind((64, 64), 3);
        assert!(binding.is_bound());
        assert!(!binding.needs_rebind((64, 64), 3));
    }

    #[test]
    fn a_changed_extent_is_owed_a_refresh() {
        let mut binding = Binding::new();
        binding.record_bind((64, 64), 3);
        assert!(binding.needs_rebind((65, 64), 3));
    }

    /// The case comparing extent alone would miss: a reap recreated the
    /// target at the identical size, so the engine is holding a view of a
    /// texture nothing writes any more.
    #[test]
    fn a_changed_generation_at_the_same_extent_is_owed_a_refresh() {
        let mut binding = Binding::new();
        binding.record_bind((64, 64), 3);
        assert!(binding.needs_rebind((64, 64), 4));
    }

    #[test]
    fn taking_a_binding_reports_whether_there_was_one_and_forces_the_next_bind() {
        let mut binding = Binding::new();
        assert!(!binding.take());
        binding.record_bind((64, 64), 3);
        assert!(binding.take());
        assert!(!binding.is_bound());
        assert!(
            binding.needs_rebind((64, 64), 3),
            "the identical target has to be bound again after an unbind"
        );
    }
}
