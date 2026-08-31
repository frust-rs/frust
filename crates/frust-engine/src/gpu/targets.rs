//! The per-renderer pool the engine draws its off-screen intermediates from.
//!
//! A layer, a filter input, a scratch copy — every target that is not the
//! frame's own surface is a short-lived texture that a later pass in the same
//! frame samples and nothing outlives the frame. Allocating one per frame is
//! the cost `frust_gpu::TexturePool` exists to remove, so the engine holds one
//! pool per renderer (one per surface, since a renderer is per surface) and
//! takes its intermediates from there.
//!
//! Two engine-level decisions sit on top of the substrate pool.
//!
//! - **A ceiling of [`MAX_INTERMEDIATE_DIMENSION`].** The adapter's own
//!   `max_texture_dimension_2d` can be far larger than anything worth
//!   allocating as a transient: a single 16384-square `Rgba8Unorm` intermediate
//!   is a gigabyte. [`max_texture_size`] therefore takes the smaller of the
//!   adapter's ceiling and 8192, which is the largest intermediate the engine
//!   will ask a driver for.
//! - **An over-ceiling request is a value, not an error.** A layer larger than
//!   that ceiling is answered with [`IntermediateTexture::TooLarge`] carrying
//!   the extent that was refused. Splitting such a layer into bands is later
//!   work; until it exists a caller can see exactly what it asked for and skip
//!   the layer rather than take a device error mid-frame.
//!
//! Like the substrate pool this type is generic over the texture and view
//! types, so its keying, reuse and aging are exercised against a counting fake
//! with no GPU in the loop. The engine always uses the default
//! `IntermediateTargets<wgpu::Texture, wgpu::TextureView>`.
//!
//! ## The one off-screen target this module does not hand out
//!
//! [`atlas_layer_config`] describes a pass whose colour attachment is one
//! layer of the glyph/image atlas array — a target [`crate::gpu::atlas`] owns
//! outright and deliberately keeps out of the pool (its module doc gives the
//! reason: residency across frames is the atlas's whole purpose). The
//! *allocation* therefore belongs there; the *viewport* decision belongs here,
//! beside [`IntermediateTargets::descriptor`], because it is the same decision
//! this module already makes for every other target that is not the frame's own
//! surface — what extent the vertex stage maps its NDC against, and which
//! resource-texture widths the fragment stage reconstructs by shift.

use frust_gpu::{PoolStats, PooledTexture, TextureAllocator, TextureDesc, TexturePool, TierCaps};

use super::config::GpuConfig;
use super::pipelines::INTERMEDIATE_FORMAT;

/// The largest intermediate the engine allocates on any adapter, whatever its
/// own `max_texture_dimension_2d` reports.
///
/// 8192 square is 256 MiB at [`INTERMEDIATE_FORMAT`] — already far past any
/// real layer — and a transient that big is a memory decision rather than a
/// capability one, which is why it is pinned here instead of taken from the
/// adapter.
pub const MAX_INTERMEDIATE_DIMENSION: u32 = 8192;

/// The usage every intermediate is created with: drawn into by a strip pass,
/// then sampled by the pass that composites it.
pub const INTERMEDIATE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// The largest intermediate extent the engine will request on `caps`' adapter.
#[must_use]
pub fn max_texture_size(caps: &TierCaps) -> u32 {
    caps.max_texture_dimension_2d
        .min(MAX_INTERMEDIATE_DIMENSION)
}

/// The viewport uniform a strip pass whose colour attachment is one atlas
/// array layer draws with.
///
/// `page` is the layer's own extent in texels — the atlas page size, not the
/// frame's — because the vertex stage maps a strip's pixel coordinates into NDC
/// against the *attachment* it writes, and an atlas layer is neither the
/// surface's extent nor a pooled page's. Getting this wrong does not fail
/// validation: it silently scales every glyph by the ratio of the two extents.
///
/// `alphas_tex_width` is the coverage texture the replayed strips sample;
/// `encoded_paints_tex_width` is the encoded-paint texture, which an atlas pass
/// only ever binds as a stand-in — a replayed glyph outline paints solid, and a
/// solid instance carries its colour in its own payload rather than indexing a
/// record. Both must be powers of two, for
/// [`tex_width_bits`](super::config::tex_width_bits)' reason.
///
/// No strip offset and no NDC negation: an atlas page holds its glyphs at the
/// slot coordinates the allocator handed out, in the same y-down space every
/// other engine target uses.
#[must_use]
pub fn atlas_layer_config(
    page: (u32, u32),
    alphas_tex_width: u32,
    encoded_paints_tex_width: u32,
) -> GpuConfig {
    GpuConfig::new(page.0, page.1, alphas_tex_width, encoded_paints_tex_width)
}

/// Whether an atlas page of `page` texels can be a render attachment on
/// `caps`' adapter.
///
/// The atlas is not pooled, so it never passes through
/// [`IntermediateTargets::acquire`]'s own ceiling check — but a page is still a
/// texture a driver has to accept, and the budgets
/// [`crate::cache::images::AtlasBudget`] hands out are chosen without the
/// adapter in view. This is the check a caller makes once, at the point it
/// decides a page size, rather than discovering the refusal as a device error
/// on the first frame that misses a glyph.
///
/// The adapter's own `max_texture_dimension_2d` is the bound, not
/// [`MAX_INTERMEDIATE_DIMENSION`]: that ceiling is a transient-memory decision
/// about targets allocated per frame, and an atlas page is allocated once and
/// lives for the renderer.
#[must_use]
pub fn atlas_page_fits(caps: &TierCaps, page: (u32, u32)) -> bool {
    page.0 > 0
        && page.1 > 0
        && page.0 <= caps.max_texture_dimension_2d
        && page.1 <= caps.max_texture_dimension_2d
}

/// The result of asking for an intermediate: a pooled texture, or the extent
/// that was refused.
#[derive(Debug)]
pub enum IntermediateTexture<T = wgpu::Texture, V = wgpu::TextureView> {
    /// A texture at least the requested extent. Its allocation is quantized
    /// up, so render into `PooledTexture::requested_size`, not `size`.
    Texture(PooledTexture<T, V>),
    /// The request exceeded [`IntermediateTargets::max_texture_size`] on at
    /// least one axis and nothing was allocated.
    TooLarge {
        /// Requested width in texels.
        width: u32,
        /// Requested height in texels.
        height: u32,
        /// The per-axis ceiling that refused it.
        max: u32,
    },
}

impl<T, V> IntermediateTexture<T, V> {
    /// The pooled texture, or `None` for a refused request.
    #[must_use]
    pub fn texture(&self) -> Option<&PooledTexture<T, V>> {
        match self {
            Self::Texture(texture) => Some(texture),
            Self::TooLarge { .. } => None,
        }
    }

    /// Whether the request was refused for exceeding the ceiling.
    #[must_use]
    pub fn is_too_large(&self) -> bool {
        matches!(self, Self::TooLarge { .. })
    }
}

/// The engine's pool of off-screen intermediates, with its own frame clock.
///
/// The clock is what the substrate pool ages entries against; it advances once
/// per [`Self::end_frame`], including frames that acquired nothing — those are
/// the frames a parked entry ages on.
#[derive(Debug)]
pub struct IntermediateTargets<T = wgpu::Texture, V = wgpu::TextureView> {
    pool: TexturePool<T, V>,
    max_texture_size: u32,
    frame: u64,
}

impl<T, V> IntermediateTargets<T, V> {
    /// A pool sized for `caps`' adapter.
    #[must_use]
    pub fn new(caps: &TierCaps) -> Self {
        Self {
            pool: TexturePool::new(caps),
            max_texture_size: max_texture_size(caps),
            frame: 0,
        }
    }

    /// The largest intermediate this pool will allocate, per axis.
    #[must_use]
    pub fn max_texture_size(&self) -> u32 {
        self.max_texture_size
    }

    /// The frame clock parked entries age against.
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// The underlying pool's counters.
    #[must_use]
    pub fn stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// The descriptor an intermediate of `width` x `height` is requested with,
    /// before the pool quantizes it.
    #[must_use]
    pub fn descriptor(width: u32, height: u32, label: &str) -> TextureDesc {
        TextureDesc {
            width,
            height,
            format: INTERMEDIATE_FORMAT,
            usage: INTERMEDIATE_USAGE,
            label: Some(label.to_string()),
        }
    }

    /// Hands out an intermediate of at least `width` x `height`, or refuses it
    /// for exceeding [`Self::max_texture_size`].
    ///
    /// The check is on the *requested* extent rather than the quantized one:
    /// the substrate pool clamps its quantization to the adapter ceiling
    /// already, so a request inside the engine's ceiling can never quantize
    /// past the adapter's.
    pub fn acquire<A>(
        &mut self,
        allocator: &A,
        width: u32,
        height: u32,
        label: &str,
    ) -> IntermediateTexture<T, V>
    where
        A: TextureAllocator<Texture = T, View = V>,
    {
        if width > self.max_texture_size || height > self.max_texture_size {
            return IntermediateTexture::TooLarge {
                width,
                height,
                max: self.max_texture_size,
            };
        }

        let desc = Self::descriptor(width, height, label);
        IntermediateTexture::Texture(self.pool.acquire(allocator, &desc, self.frame))
    }

    /// Parks an intermediate for reuse.
    pub fn release(&mut self, texture: PooledTexture<T, V>) {
        self.pool.release(texture);
    }

    /// Advances the frame clock and ages parked entries out of the pool.
    ///
    /// Call once per frame, whether or not the frame acquired anything.
    pub fn end_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
        self.pool.age(self.frame);
    }

    /// Drops every parked entry, keeping the pool itself and its counters.
    ///
    /// This is what a surface resize needs: every parked entry is keyed on an
    /// extent nothing will ask for again, so holding them to the end of the
    /// keep-alive window is pure waste. The clock is advanced past that window
    /// rather than the entries being reached into directly, which is the same
    /// eviction the pool would perform on its own a second later. Entries still
    /// checked out are untouched — the pool does not hold those.
    pub fn drop_parked(&mut self) {
        self.frame = self
            .frame
            .saturating_add(self.pool.max_unused_frames().saturating_add(1));
        self.pool.age(self.frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use frust_gpu::DownlevelProfile;

    /// A [`TextureAllocator`] with no GPU behind it: the texture and view are
    /// both the allocation's ordinal, so "how many textures did this actually
    /// allocate?" is answerable with no device.
    #[derive(Debug, Default)]
    struct FakeAllocator {
        allocations: Cell<u32>,
    }

    impl TextureAllocator for FakeAllocator {
        type Texture = u32;
        type View = u32;

        fn allocate_texture(&self, _desc: &TextureDesc) -> (u32, u32) {
            self.allocations.set(self.allocations.get() + 1);
            (self.allocations.get(), self.allocations.get())
        }
    }

    fn caps() -> TierCaps {
        TierCaps::fake(DownlevelProfile::Full)
    }

    fn targets() -> IntermediateTargets<u32, u32> {
        IntermediateTargets::new(&caps())
    }

    #[test]
    fn the_ceiling_is_the_smaller_of_the_adapter_limit_and_the_engine_cap() {
        let mut caps = caps();
        caps.max_texture_dimension_2d = 16384;
        assert_eq!(max_texture_size(&caps), MAX_INTERMEDIATE_DIMENSION);

        caps.max_texture_dimension_2d = 4096;
        assert_eq!(max_texture_size(&caps), 4096);
    }

    #[test]
    fn an_intermediate_is_a_sampled_copyable_render_attachment() {
        let desc = IntermediateTargets::<u32, u32>::descriptor(64, 32, "layer");
        assert_eq!(desc.format, INTERMEDIATE_FORMAT);
        assert!(desc.usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT));
        assert!(desc.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert_eq!(desc.sample_count(), 1);
    }

    #[test]
    fn an_atlas_layer_pass_maps_its_ndc_against_the_page_not_the_frame() {
        let config = atlas_layer_config((1024, 1024), 2048, 1);

        assert_eq!(config.width, 1024);
        assert_eq!(config.height, 1024);
        assert_eq!(config.strip_offset_x, 0);
        assert_eq!(config.strip_offset_y, 0);
        assert_eq!(config.negate_ndc, 0);
        assert_eq!(config.alphas_tex_width_bits, 11, "log2(2048)");
        assert_eq!(
            config.encoded_paints_tex_width_bits, 0,
            "a 1-texel stand-in reconstructs as `1 << 0`"
        );
    }

    #[test]
    fn a_page_past_the_adapters_own_limit_does_not_fit() {
        let mut caps = caps();
        caps.max_texture_dimension_2d = 2048;

        assert!(atlas_page_fits(&caps, (2048, 2048)));
        assert!(!atlas_page_fits(&caps, (4096, 1024)));
        assert!(!atlas_page_fits(&caps, (1024, 4096)));
        // A degenerate page is not a page: nothing can be allocated in it, and
        // a zero-extent attachment is a device error rather than an empty pass.
        assert!(!atlas_page_fits(&caps, (0, 1024)));
        assert!(!atlas_page_fits(&caps, (1024, 0)));
    }

    #[test]
    fn the_atlas_pages_ceiling_is_the_adapters_rather_than_the_transient_cap() {
        let mut caps = caps();
        caps.max_texture_dimension_2d = 16384;

        // The pool refuses this; the atlas does not, because a page is
        // allocated once for the renderer rather than per frame.
        const { assert!(MAX_INTERMEDIATE_DIMENSION < 16384) };
        assert!(atlas_page_fits(&caps, (16384, 16384)));
        assert_eq!(max_texture_size(&caps), MAX_INTERMEDIATE_DIMENSION);
    }

    #[test]
    fn an_oversized_request_is_refused_without_allocating() {
        let allocator = FakeAllocator::default();
        let mut targets = targets();
        let max = targets.max_texture_size();

        let refused = targets.acquire(&allocator, max + 1, 16, "huge layer");
        assert!(refused.is_too_large());
        assert!(refused.texture().is_none());
        assert!(matches!(
            refused,
            IntermediateTexture::TooLarge { width, height, max: ceiling }
                if width == max + 1 && height == 16 && ceiling == max
        ));
        assert_eq!(allocator.allocations.get(), 0);
        assert_eq!(targets.stats().created, 0);

        // The other axis is checked the same way.
        assert!(
            targets
                .acquire(&allocator, 16, max + 1, "huge layer")
                .is_too_large()
        );
        assert_eq!(allocator.allocations.get(), 0);
    }

    #[test]
    fn a_released_intermediate_is_reused_rather_than_reallocated() {
        let allocator = FakeAllocator::default();
        let mut targets = targets();

        let first = targets.acquire(&allocator, 300, 200, "layer");
        let pooled = match first {
            IntermediateTexture::Texture(texture) => texture,
            IntermediateTexture::TooLarge { .. } => unreachable!("300x200 is inside the ceiling"),
        };
        // Quantized up to the pool's 256-px keys, with the request preserved.
        assert_eq!(pooled.requested_size(), (300, 200));
        assert_eq!(pooled.size(), (512, 256));
        targets.release(pooled);

        let second = targets.acquire(&allocator, 300, 200, "layer");
        assert!(second.texture().is_some());
        assert_eq!(allocator.allocations.get(), 1, "the second acquire reuses");
        assert_eq!(targets.stats().reused, 1);
    }

    #[test]
    fn a_parked_entry_ages_out_after_the_keep_alive_window() {
        let allocator = FakeAllocator::default();
        let mut targets = targets();

        let pooled = targets
            .acquire(&allocator, 256, 256, "layer")
            .texture()
            .is_some();
        assert!(pooled);
        let entry = match targets.acquire(&allocator, 256, 256, "layer") {
            IntermediateTexture::Texture(texture) => texture,
            IntermediateTexture::TooLarge { .. } => unreachable!("256x256 is inside the ceiling"),
        };
        targets.release(entry);
        assert_eq!(targets.stats().free, 1);

        // Frames that acquire nothing are the frames entries age on.
        for _ in 0..60 {
            targets.end_frame();
        }
        assert_eq!(
            targets.stats().free,
            1,
            "still inside the keep-alive window"
        );

        targets.end_frame();
        assert_eq!(targets.stats().free, 0);
        assert_eq!(targets.stats().evicted, 1);
        assert_eq!(targets.frame(), 61);
    }

    #[test]
    fn dropping_parked_entries_clears_the_pool_at_once() {
        let allocator = FakeAllocator::default();
        let mut targets = targets();

        for size in [256_u32, 512, 768] {
            match targets.acquire(&allocator, size, size, "layer") {
                IntermediateTexture::Texture(texture) => targets.release(texture),
                IntermediateTexture::TooLarge { .. } => unreachable!("inside the ceiling"),
            }
        }
        assert_eq!(targets.stats().free, 3);
        assert_eq!(targets.stats().keys, 3);

        targets.drop_parked();

        assert_eq!(targets.stats().free, 0);
        assert_eq!(targets.stats().keys, 0);
        assert_eq!(targets.stats().evicted, 3);
        // The pool itself survives: a fresh acquire still works, and the
        // lifetime counters are not reset by the eviction.
        assert_eq!(targets.stats().created, 3);
        assert!(
            targets
                .acquire(&allocator, 256, 256, "layer")
                .texture()
                .is_some()
        );
        assert_eq!(targets.stats().created, 4);
    }

    #[test]
    fn a_checked_out_intermediate_survives_a_drop_of_the_parked_entries() {
        let allocator = FakeAllocator::default();
        let mut targets = targets();

        let held = match targets.acquire(&allocator, 256, 256, "layer") {
            IntermediateTexture::Texture(texture) => texture,
            IntermediateTexture::TooLarge { .. } => unreachable!("inside the ceiling"),
        };
        targets.drop_parked();
        assert_eq!(targets.stats().evicted, 0, "nothing was parked to evict");
        assert_eq!(targets.stats().in_use, 1);

        // It returns to the pool as usual afterwards.
        targets.release(held);
        assert_eq!(targets.stats().free, 1);
    }
}
