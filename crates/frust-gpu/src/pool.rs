//! A recycling pool for the short-lived textures a frame renders into:
//! [`TexturePool`], the [`PooledTexture`] it hands out, and the
//! [`TextureAllocator`] seam that keeps both host-testable.
//!
//! An intermediate render target — a scratch surface a pass draws into and a
//! later pass consumes — is allocated and dropped every frame if nothing
//! recycles it. That is exactly the cost the hybrid renderer's own TODO
//! names ("we currently allocate a new strips buffer for each render pass"),
//! and it is worst during a resize storm: dragging a window edge produces a
//! new surface size every frame, so a pool keyed on the exact requested
//! extent allocates a fresh texture per frame and parks the previous one
//! forever.
//!
//! Two decisions make the pool survive that storm.
//!
//! - **Quantization.** A request's extent is rounded up to the next multiple
//!   of [`SIZE_QUANTUM`] (256 px) *before* it becomes a key, so a drag
//!   through 4096 distinct widths lands on a couple of dozen keys instead.
//!   The caller gets a texture at least as large as it asked for and renders
//!   into the sub-rect it actually wants ([`PooledTexture::requested_size`]);
//!   Impeller's coverage-size quantization is the same trade — a bounded
//!   amount of slack memory in exchange for reuse across a size that never
//!   stops changing.
//! - **Aging with slack.** A free entry survives
//!   [`DEFAULT_MAX_UNUSED_FRAMES`] (60) frames of disuse before
//!   [`TexturePool::age`] drops it. `frust-render`'s compositor scratch ages
//!   out after 2 frames, which is right for one full-surface texture whose
//!   size is pinned to the surface, and far too aggressive here: a resize
//!   drag revisits a quantized size seconds later, and a two-frame window
//!   would evict every entry between visits and turn the pool back into a
//!   plain allocator. Grow-and-never-shrink is the other failure mode, so the
//!   window is finite and tunable ([`TexturePool::with_max_unused_frames`]).
//!
//! # Recycled contents are never loaded
//!
//! A pooled texture's prior contents belong to whichever pass used it last,
//! so a pass targeting one must CLEAR, never LOAD — that is what
//! [`PooledTexture::color_attachment`] builds, and it is a correctness rule
//! before it is a bandwidth one. Pairing `LoadOp::Clear` with
//! `StoreOp::Discard` is also the precondition for
//! `wgpu::TextureUsages::TRANSIENT_ATTACHMENT`, which [`effective_usage`]
//! adds on an adapter that reports [`TierCaps::transient_saves_memory`] — a
//! write-only attachment can then live in tile memory and may never get
//! backing storage at all. `TRANSIENT_ATTACHMENT` is incompatible with every
//! other usage, so it is applied only to a texture whose whole usage set is
//! `RENDER_ATTACHMENT`; anything sampled or copied out afterwards keeps the
//! clear/discard policy and no flag.
//!
//! # No GPU in the loop
//!
//! Allocation goes through [`TextureAllocator`] rather than a `wgpu::Device`
//! borrow, so the pool's keying, reuse, aging and statistics are exercised
//! against a counting fake with no device — the same pure-decision /
//! platform-lookup split [`crate::caps::TierCaps::fake`] gives adapter
//! capabilities. `wgpu::Device` implements the trait, and it is the only
//! implementation the engine itself ever passes.

use std::collections::HashMap;

use crate::caps::TierCaps;
use crate::texture::{ColorAttachment, Texture, TextureDesc};

/// The multiple every pooled texture's width and height is rounded up to
/// before it is used as a pool key.
///
/// 256 px is Impeller's coverage quantum: coarse enough that a resize drag
/// collapses onto a handful of keys, fine enough that the slack area stays a
/// small fraction of a real surface (a 5120x2880 request grows to
/// 5120x3072 — 6.7% extra).
pub const SIZE_QUANTUM: u32 = 256;

/// How many frames a free entry may go unused before [`TexturePool::age`]
/// drops it: roughly a second of frames at 60 Hz, so a resize drag that
/// revisits a quantized size still finds it parked.
pub const DEFAULT_MAX_UNUSED_FRAMES: u64 = 60;

/// Creates the texture/view pair a [`TexturePool`] hands out.
///
/// Exists so the pool takes an allocation *capability* instead of a
/// `wgpu::Device`: a host test implements it with a counter and plain
/// stand-in values, which is what makes "how many textures did 400 resizes
/// actually allocate?" answerable with no GPU. `wgpu::Device` is the only
/// production implementation.
pub trait TextureAllocator {
    /// The allocated texture — `wgpu::Texture` in production.
    type Texture;
    /// A full-extent view of [`Self::Texture`] — `wgpu::TextureView` in
    /// production.
    type View;

    /// Creates a texture matching `desc` plus a full-extent view of it.
    ///
    /// Named `allocate_texture` rather than `create_texture` so it never
    /// shadows `wgpu::Device`'s inherent method of that name.
    fn allocate_texture(&self, desc: &TextureDesc) -> (Self::Texture, Self::View);
}

impl TextureAllocator for wgpu::Device {
    type Texture = wgpu::Texture;
    type View = wgpu::TextureView;

    fn allocate_texture(&self, desc: &TextureDesc) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = self.create_texture(&wgpu::TextureDescriptor {
            label: desc.label.as_deref(),
            size: wgpu::Extent3d {
                width: desc.width,
                height: desc.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: desc.sample_count(),
            dimension: wgpu::TextureDimension::D2,
            format: desc.format,
            usage: desc.usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }
}

/// The configuration a pooled texture is interchangeable within: two
/// requests that produce the same key may share one texture, and two that do
/// not never can.
///
/// `width`/`height` are the **quantized** extent ([`quantize_extent`]), and
/// `usage` is the **effective** usage ([`effective_usage`]) rather than the
/// requested one, because those are what the pooled texture was actually
/// created with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PoolKey {
    /// Quantized width in texels.
    pub width: u32,
    /// Quantized height in texels.
    pub height: u32,
    /// The texture's pixel format.
    pub format: wgpu::TextureFormat,
    /// The usage the texture was created with.
    pub usage: wgpu::TextureUsages,
    /// The texture's sample count. Always 1 today ([`TextureDesc`] exposes
    /// no way to ask for more), and part of the key anyway so a multisampled
    /// target could never alias a single-sampled one.
    pub sample_count: u32,
}

/// A texture checked out of a [`TexturePool`], the key it returns under, and
/// the frame clock it ages against.
///
/// Held by value while in use — the pool retains only free entries — so a
/// caller that forgets to [`TexturePool::release`] it simply drops it, at
/// worst losing the reuse rather than leaking a slot.
#[derive(Clone, Debug)]
pub struct PooledTexture<T = wgpu::Texture, V = wgpu::TextureView> {
    texture: Texture<T, V>,
    key: PoolKey,
    last_used_frame: u64,
    requested: (u32, u32),
}

impl<T, V> PooledTexture<T, V> {
    /// The pooled texture itself, carrying the [`TextureDesc`] it was
    /// created from (the *quantized* extent) and its
    /// [`crate::texture::SceneTextureId`].
    pub fn texture(&self) -> &Texture<T, V> {
        &self.texture
    }

    /// The texture's full-extent view.
    pub fn view(&self) -> &V {
        self.texture.view()
    }

    /// The quantized `(width, height)` this texture was allocated at — at
    /// least [`Self::requested_size`] in both axes.
    pub fn size(&self) -> (u32, u32) {
        self.texture.size()
    }

    /// The `(width, height)` the current holder asked for, which is what it
    /// should render into: the allocation is quantized up, so the rest of
    /// the texture is slack the caller must not treat as part of its image.
    pub fn requested_size(&self) -> (u32, u32) {
        self.requested
    }

    /// The configuration this entry returns to the pool under.
    pub fn key(&self) -> PoolKey {
        self.key
    }

    /// The frame this entry was last acquired for — the clock
    /// [`TexturePool::age`] measures disuse against.
    pub fn last_used_frame(&self) -> u64 {
        self.last_used_frame
    }
}

impl PooledTexture<wgpu::Texture, wgpu::TextureView> {
    /// A color attachment targeting this texture, clearing to `clear` and
    /// discarding at pass end.
    ///
    /// The ops are not a parameter on purpose. A recycled texture holds
    /// whatever its previous holder left there, so loading it would sample
    /// another pass's image; and `StoreOp::Discard` is required outright once
    /// [`effective_usage`] has added
    /// `wgpu::TextureUsages::TRANSIENT_ATTACHMENT`. A caller that needs its
    /// own contents back across passes wants a texture it owns, not a pooled
    /// intermediate.
    pub fn color_attachment(&self, clear: wgpu::Color) -> ColorAttachment<'_> {
        ColorAttachment {
            view: self.texture.view(),
            load: wgpu::LoadOp::Clear(clear),
            store: wgpu::StoreOp::Discard,
        }
    }
}

/// A [`TexturePool`]'s counters, as of the [`TexturePool::stats`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Textures the pool has ever asked the allocator for. The number a
    /// resize-storm test holds against a budget.
    pub created: u64,
    /// Acquires satisfied from a free entry instead of an allocation.
    pub reused: u64,
    /// Entries dropped by [`TexturePool::age`].
    pub evicted: u64,
    /// Entries acquired and not yet released.
    pub in_use: u64,
    /// Entries currently parked and reusable.
    pub free: usize,
    /// Distinct [`PoolKey`]s with at least one parked entry.
    pub keys: usize,
}

/// Recycles the intermediate textures a frame renders into, keyed by
/// quantized configuration and aged by frame clock.
///
/// Generic over the texture (`T`) and view (`V`) types for the same reason
/// [`Texture`] is: the pool's own behavior is exercised on a host with plain
/// stand-in values. The engine always uses the default
/// `TexturePool<wgpu::Texture, wgpu::TextureView>`.
#[derive(Debug)]
pub struct TexturePool<T = wgpu::Texture, V = wgpu::TextureView> {
    free: HashMap<PoolKey, Vec<PooledTexture<T, V>>>,
    max_dimension: u32,
    transient_saves_memory: bool,
    max_unused_frames: u64,
    created: u64,
    reused: u64,
    evicted: u64,
    in_use: u64,
}

impl<T, V> TexturePool<T, V> {
    /// An empty pool configured from an adapter's capabilities: quantized
    /// extents are capped at [`TierCaps::max_texture_dimension_2d`] and
    /// `wgpu::TextureUsages::TRANSIENT_ATTACHMENT` is applied only where
    /// [`TierCaps::transient_saves_memory`] says it buys something.
    pub fn new(caps: &TierCaps) -> Self {
        Self::with_max_unused_frames(caps, DEFAULT_MAX_UNUSED_FRAMES)
    }

    /// [`Self::new`] with an explicit keep-alive window, for a host that
    /// knows its own memory budget is tighter (or its resize storms longer)
    /// than the [`DEFAULT_MAX_UNUSED_FRAMES`] default.
    pub fn with_max_unused_frames(caps: &TierCaps, max_unused_frames: u64) -> Self {
        Self {
            free: HashMap::new(),
            max_dimension: caps.max_texture_dimension_2d,
            transient_saves_memory: caps.transient_saves_memory,
            max_unused_frames,
            created: 0,
            reused: 0,
            evicted: 0,
            in_use: 0,
        }
    }

    /// The [`PoolKey`] `desc` resolves to: quantized extent, effective
    /// usage, format and sample count.
    ///
    /// Public because a caller sizing a cache or asserting that two requests
    /// share a texture should ask the pool rather than re-deriving the
    /// quantization rule.
    pub fn key_for(&self, desc: &TextureDesc) -> PoolKey {
        let (width, height) = quantize_extent(desc.width, desc.height, self.max_dimension);
        PoolKey {
            width,
            height,
            format: desc.format,
            usage: effective_usage(desc.usage, self.transient_saves_memory),
            sample_count: desc.sample_count(),
        }
    }

    /// Hands out a texture for `desc`, reusing a parked entry of the same
    /// configuration when there is one and allocating otherwise.
    ///
    /// `frame` is the caller's monotonically increasing frame counter; it is
    /// stamped on the entry and is what [`Self::age`] measures disuse
    /// against. The returned texture is at least `desc`'s extent and usually
    /// larger — see [`PooledTexture::requested_size`].
    pub fn acquire<A>(
        &mut self,
        allocator: &A,
        desc: &TextureDesc,
        frame: u64,
    ) -> PooledTexture<T, V>
    where
        A: TextureAllocator<Texture = T, View = V>,
    {
        let key = self.key_for(desc);
        let requested = (desc.width, desc.height);
        self.in_use += 1;

        if let Some(mut entry) = self.free.get_mut(&key).and_then(Vec::pop) {
            if self.free.get(&key).is_some_and(Vec::is_empty) {
                self.free.remove(&key);
            }
            entry.last_used_frame = frame;
            entry.requested = requested;
            self.reused += 1;
            return entry;
        }

        let pooled_desc = TextureDesc {
            width: key.width,
            height: key.height,
            format: key.format,
            usage: key.usage,
            label: desc.label.clone(),
        };
        let (texture, view) = allocator.allocate_texture(&pooled_desc);
        self.created += 1;
        PooledTexture {
            texture: Texture::new(texture, view, pooled_desc),
            key,
            last_used_frame: frame,
            requested,
        }
    }

    /// Parks `texture` for reuse under the key it was acquired with.
    ///
    /// Its [`PooledTexture::last_used_frame`] stamp is left as the acquiring
    /// frame, so the keep-alive window is measured from when the entry was
    /// last *needed*, not from when the holder happened to give it back.
    pub fn release(&mut self, texture: PooledTexture<T, V>) {
        self.in_use = self.in_use.saturating_sub(1);
        self.free.entry(texture.key).or_default().push(texture);
    }

    /// Advances the pool to `frame` and drops every parked entry unused for
    /// more than the keep-alive window.
    ///
    /// Call once per frame, including frames that acquired nothing — those
    /// are the frames entries age on. Entries still checked out are
    /// untouched: the pool does not hold them.
    pub fn age(&mut self, frame: u64) {
        let max_unused = self.max_unused_frames;
        let mut evicted = 0u64;
        self.free.retain(|_, entries| {
            let before = entries.len();
            entries.retain(|entry| !entry_expired(entry.last_used_frame, frame, max_unused));
            evicted += (before - entries.len()) as u64;
            !entries.is_empty()
        });
        self.evicted += evicted;
    }

    /// The pool's counters right now.
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            created: self.created,
            reused: self.reused,
            evicted: self.evicted,
            in_use: self.in_use,
            free: self.free.values().map(Vec::len).sum(),
            keys: self.free.len(),
        }
    }

    /// The keep-alive window in frames, as configured.
    pub fn max_unused_frames(&self) -> u64 {
        self.max_unused_frames
    }
}

/// Rounds `width` and `height` up to the next multiple of [`SIZE_QUANTUM`],
/// never past `max_dimension`.
///
/// The clamp keeps quantization from pushing a request that already sits
/// just under the adapter's ceiling over it. A dimension that is *itself*
/// above `max_dimension` is passed through unchanged rather than silently
/// shrunk, so the device reports the real error instead of the pool handing
/// back a texture that is not the size anyone asked for. A zero extent is
/// likewise passed through — it is a caller bug, and the device names it.
pub fn quantize_extent(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    (
        quantize_dimension(width, max_dimension),
        quantize_dimension(height, max_dimension),
    )
}

fn quantize_dimension(value: u32, max_dimension: u32) -> u32 {
    value
        .checked_next_multiple_of(SIZE_QUANTUM)
        .unwrap_or(value)
        .min(max_dimension)
        .max(value)
}

/// The usage a pooled texture requested as `usage` is actually created with.
///
/// Adds `wgpu::TextureUsages::TRANSIENT_ATTACHMENT` — which lets a driver
/// keep the texture in tile memory and possibly never back it with real
/// storage — on an adapter that reports it saves memory, and only for a
/// texture whose entire usage set is `RENDER_ATTACHMENT`. That restriction is
/// `wgpu`'s own: `TRANSIENT_ATTACHMENT` requires `RENDER_ATTACHMENT` and is
/// incompatible with every other usage, which lines up exactly with "an
/// intermediate nothing reads back". Anything sampled, copied or stored keeps
/// its usage unchanged and relies on the clear/discard ops alone.
pub fn effective_usage(
    usage: wgpu::TextureUsages,
    transient_saves_memory: bool,
) -> wgpu::TextureUsages {
    if transient_saves_memory && usage == wgpu::TextureUsages::RENDER_ATTACHMENT {
        usage | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
    } else {
        usage
    }
}

/// Whether an entry last used at `last_used` is stale at `frame`.
///
/// The boundary is inclusive of `frame - max_unused` (that entry survives),
/// the same shape `frust-render`'s compositor ages its scratch texture by —
/// only the window differs.
fn entry_expired(last_used: u64, frame: u64, max_unused: u64) -> bool {
    frame.saturating_sub(last_used) > max_unused
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::DownlevelProfile;

    #[test]
    fn quantization_rounds_up_to_the_next_multiple() {
        assert_eq!(quantize_dimension(1, 8192), 256);
        assert_eq!(quantize_dimension(255, 8192), 256);
        assert_eq!(quantize_dimension(256, 8192), 256);
        assert_eq!(quantize_dimension(257, 8192), 512);
        assert_eq!(quantize_dimension(800, 8192), 1024);
        assert_eq!(quantize_dimension(2880, 8192), 3072);
        assert_eq!(quantize_dimension(5120, 8192), 5120);
    }

    #[test]
    fn quantization_never_exceeds_the_adapter_ceiling() {
        // Rounding 2000 up would land on 2048, past a 2000-px ceiling.
        assert_eq!(quantize_dimension(2000, 2000), 2000);
        // A request already above the ceiling is passed through so the
        // device reports it, not shrunk to something nobody asked for.
        assert_eq!(quantize_dimension(4096, 2048), 4096);
    }

    #[test]
    fn quantization_passes_a_zero_extent_through() {
        assert_eq!(quantize_extent(0, 0, 8192), (0, 0));
    }

    #[test]
    fn transient_is_added_only_for_a_write_only_attachment() {
        let attachment = wgpu::TextureUsages::RENDER_ATTACHMENT;
        assert_eq!(
            effective_usage(attachment, true),
            attachment | wgpu::TextureUsages::TRANSIENT_ATTACHMENT
        );
        assert_eq!(effective_usage(attachment, false), attachment);

        // Sampled afterwards, so the contents ARE read back: no flag, on
        // either adapter.
        let sampled = attachment | wgpu::TextureUsages::TEXTURE_BINDING;
        assert_eq!(effective_usage(sampled, true), sampled);
        assert_eq!(effective_usage(sampled, false), sampled);

        // Copied out, same reasoning.
        let copied = attachment | wgpu::TextureUsages::COPY_SRC;
        assert_eq!(effective_usage(copied, true), copied);
    }

    #[test]
    fn expiry_boundary_matches_the_compositor_precedent() {
        assert!(!entry_expired(7, 7, 60));
        assert!(!entry_expired(7, 67, 60));
        assert!(entry_expired(7, 68, 60));
        // A frame clock that has not advanced past the stamp never expires.
        assert!(!entry_expired(9, 3, 60));
    }

    #[test]
    fn key_folds_quantization_and_transient_policy_together() {
        let mut caps = TierCaps::fake(DownlevelProfile::Full);
        caps.transient_saves_memory = true;
        let pool: TexturePool<u32, u32> = TexturePool::new(&caps);

        let desc = TextureDesc {
            width: 900,
            height: 700,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            label: None,
        };
        let key = pool.key_for(&desc);
        assert_eq!((key.width, key.height), (1024, 768));
        assert!(
            key.usage
                .contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT)
        );
        assert_eq!(key.sample_count, 1);
    }

    #[test]
    fn stats_start_empty() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        let pool: TexturePool<u32, u32> = TexturePool::new(&caps);
        assert_eq!(pool.stats(), PoolStats::default());
        assert_eq!(pool.max_unused_frames(), DEFAULT_MAX_UNUSED_FRAMES);
    }
}
