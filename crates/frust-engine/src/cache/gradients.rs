//! Gradient colour-ramp (LUT) cache.
//!
//! A gradient is drawn by sampling a pre-baked colour ramp. Baking a ramp is
//! expensive relative to a frame, and the same gradient typically recurs across
//! frames (a themed button, a scrim), so ramps are cached by the colour-affecting
//! properties of the gradient — its stops, interpolation colour space and hue
//! direction — which is exactly what [`GradientCacheKey`] captures. Geometry
//! (start/end points, radii, angles) does not affect the ramp and is therefore
//! deliberately absent from the key: two gradients that differ only in placement
//! share one ramp.
//!
//! Every cached ramp lives in one packed `Rgba8Unorm` byte buffer that is uploaded
//! to a single texture as a flat texel stream. A cached ramp is addressed by a
//! texel offset plus a width, so the buffer must stay contiguous: eviction
//! therefore compacts the buffer and rewrites the offsets of the survivors.
//!
//! Residency is bounded by [`GradientCache::capacity`] and reclaimed LRU by frame
//! epoch. The epoch advances on every lookup, so each live entry carries a
//! distinct last-used value and the eviction threshold is unambiguous.

use std::collections::HashMap;

use vello_common::encode::{EncodedGradient, GradientCacheKey, MAX_GRADIENT_LUT_SIZE};
use vello_common::fearless_simd::{Level, Simd, dispatch};
use vello_common::peniko::color::cache_key::CacheKey;

/// Bytes per texel of the gradient LUT texture (`Rgba8Unorm`).
///
/// Converts between the byte offsets the packed buffer is indexed by and the
/// texel offsets a shader samples with.
pub const BYTES_PER_TEXEL: u32 = 4;

/// Geometry of the single texture the packed gradient LUTs are uploaded into.
///
/// The texture holds ramps as a flat texel stream that wraps at `width`; a ramp is
/// free to straddle a row boundary, so `width` constrains nothing but the upload
/// footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GradientTextureLayout {
    /// Texture width in texels.
    pub width: u32,
    /// Texture height in texels.
    pub height: u32,
}

impl GradientTextureLayout {
    /// The texture format gradient LUTs are packed for.
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// Create a layout for a square texture of `dim` texels a side.
    pub fn square(dim: u32) -> Self {
        Self {
            width: dim,
            height: dim,
        }
    }

    /// Row stride in bytes, as required by `wgpu`'s texel copy layout.
    pub fn bytes_per_row(self) -> u32 {
        self.width << 2
    }

    /// Total byte footprint of the texture — the length an upload must be padded to.
    pub fn byte_capacity(self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_TEXEL as usize
    }

    /// The number of cache entries this texture can hold in the worst case, where
    /// every ramp is baked at the maximum LUT size.
    pub fn worst_case_entry_capacity(self) -> u32 {
        let texels = self.width.saturating_mul(self.height);
        texels / MAX_GRADIENT_LUT_SIZE as u32
    }
}

/// Where one cached gradient ramp lives in the packed LUT buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedRamp {
    /// Texel offset at which this ramp starts.
    pub lut_start: u32,
    /// Width of this ramp in texels.
    pub width: u32,
}

/// One cache entry: a ramp plus the epoch it was last used at.
#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    ramp: CachedRamp,
    last_used: u64,
}

/// Reusable working memory for eviction, kept so that a frame that evicts does not
/// also allocate.
#[derive(Debug, Default)]
struct ScratchSpace {
    /// Last-used epochs of every live entry, used to select the eviction threshold.
    epochs: Vec<u64>,
    /// Ramps removed by the current eviction, sorted by `lut_start` for compaction.
    removed: Vec<CachedRamp>,
    /// Prefix sums of removed widths, used to rewrite surviving offsets.
    prefix_sum: Vec<u32>,
}

/// An LRU cache of baked gradient colour ramps packed into one upload buffer.
#[derive(Debug)]
pub struct GradientCache {
    /// Monotonic counter advanced on every lookup; supplies LRU ordering.
    epoch: u64,
    /// Ramps by colour-affecting gradient identity.
    entries: HashMap<CacheKey<GradientCacheKey>, CacheEntry>,
    /// All live ramps, packed contiguously as `Rgba8Unorm` texels.
    luts: Vec<u8>,
    /// Whether `luts` has changed since the last [`GradientCache::mark_synced`].
    has_changed: bool,
    /// Maximum number of entries retained across a [`GradientCache::maintain`].
    capacity: u32,
    /// SIMD level used to bake ramps.
    level: Level,
    scratch: ScratchSpace,
}

impl GradientCache {
    /// Create a cache retaining at most `capacity` ramps.
    pub fn new(capacity: u32, level: Level) -> Self {
        Self {
            epoch: 0,
            entries: HashMap::new(),
            luts: Vec::new(),
            has_changed: false,
            capacity,
            level,
            scratch: ScratchSpace::default(),
        }
    }

    /// Create a cache sized for the texture the ramps will be uploaded into.
    ///
    /// Capacity is the worst-case entry count for that texture — every ramp baked
    /// at [`MAX_GRADIENT_LUT_SIZE`] — so the packed buffer cannot outgrow the
    /// texture regardless of how complex the cached gradients turn out to be.
    pub fn for_texture(layout: GradientTextureLayout, level: Level) -> Self {
        Self::new(layout.worst_case_entry_capacity(), level)
    }

    /// The maximum number of entries retained across a [`GradientCache::maintain`].
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Number of ramps currently resident.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Size of the packed LUT buffer in bytes.
    pub fn luts_size(&self) -> usize {
        self.luts.len()
    }

    /// The packed LUT bytes, ready to be uploaded as `Rgba8Unorm` texels.
    pub fn luts(&self) -> &[u8] {
        &self.luts
    }

    /// Whether no ramp bytes are packed.
    pub fn is_empty(&self) -> bool {
        self.luts.is_empty()
    }

    /// Whether the packed bytes have changed since the last upload.
    pub fn has_changed(&self) -> bool {
        self.has_changed
    }

    /// Record that the packed bytes have been uploaded.
    pub fn mark_synced(&mut self) {
        self.has_changed = false;
    }

    /// Look a gradient up without baking it or disturbing LRU order.
    pub fn lookup(&self, gradient: &EncodedGradient) -> Option<CachedRamp> {
        self.entries
            .get(&gradient.cache_key)
            .map(|entry| entry.ramp)
    }

    /// Return the cached ramp for `gradient`, baking and packing it on a miss.
    ///
    /// Offsets returned within one frame stay valid for that frame: baking only
    /// appends, and the compaction that rewrites offsets happens in
    /// [`GradientCache::maintain`] at the frame boundary.
    pub fn get_or_create_ramp(&mut self, gradient: &EncodedGradient) -> CachedRamp {
        self.epoch += 1;

        if let Some(entry) = self.entries.get_mut(&gradient.cache_key) {
            entry.last_used = self.epoch;
            return entry.ramp;
        }

        let lut_start = u32::try_from(self.luts.len()).unwrap_or(u32::MAX) / BYTES_PER_TEXEL;
        let width = dispatch!(self.level, simd => bake_ramp(simd, gradient, &mut self.luts));
        let ramp = CachedRamp {
            lut_start,
            width: u32::try_from(width).unwrap_or(u32::MAX),
        };

        self.has_changed = true;
        self.entries.insert(
            gradient.cache_key.clone(),
            CacheEntry {
                ramp,
                last_used: self.epoch,
            },
        );

        ramp
    }

    /// Evict least-recently-used ramps down to [`GradientCache::capacity`].
    ///
    /// Call once per frame, after the frame's paints have been encoded — never
    /// mid-frame, since compaction invalidates previously returned offsets.
    pub fn maintain(&mut self) {
        let excess = self.entries.len().saturating_sub(self.capacity as usize);
        self.evict(excess);
    }

    /// Take the packed bytes, leaving the cache's buffer empty.
    ///
    /// Paired with [`GradientCache::restore_luts`] so an upload can pad the buffer
    /// to the texture footprint and hand it back without copying the ramp bytes.
    /// The restored buffer must hold the same logical content.
    pub fn take_luts(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.luts)
    }

    /// Give back a buffer taken by [`GradientCache::take_luts`].
    pub fn restore_luts(&mut self, luts: Vec<u8>) {
        self.luts = luts;
    }

    /// Borrow the packed bytes padded out to `layout`'s full byte footprint,
    /// ready to hand to a texel copy.
    ///
    /// Returns `None` when no ramps are packed. The padding is applied to the
    /// cache's own buffer and undone when the returned [`LutUpload`] drops, so an
    /// upload costs one resize rather than a copy of every ramp.
    pub fn begin_upload(&mut self, layout: GradientTextureLayout) -> Option<LutUpload<'_>> {
        if self.luts.is_empty() {
            return None;
        }

        let mut bytes = self.take_luts();
        let logical_len = bytes.len();
        bytes.resize(layout.byte_capacity(), 0);

        Some(LutUpload {
            cache: self,
            bytes,
            logical_len,
            layout,
        })
    }

    /// Remove `count` least-recently-used entries and compact the packed buffer.
    fn evict(&mut self, count: usize) {
        if count == 0 || self.entries.is_empty() {
            return;
        }

        let mut epochs = std::mem::take(&mut self.scratch.epochs);
        epochs.clear();
        epochs.extend(self.entries.values().map(|entry| entry.last_used));

        // The epoch advances on every lookup, so no two live entries share a
        // last-used value and everything at or below the threshold is exactly the
        // `count` oldest.
        let (_, &mut threshold, _) = epochs.select_nth_unstable(count - 1);
        self.scratch.epochs = epochs;

        let mut removed = std::mem::take(&mut self.scratch.removed);
        removed.clear();
        self.entries.retain(|_, entry| {
            if entry.last_used <= threshold {
                removed.push(entry.ramp);
                false
            } else {
                true
            }
        });

        removed.sort_unstable_by_key(|ramp| ramp.lut_start);
        let mut prefix_sum = std::mem::take(&mut self.scratch.prefix_sum);
        self.compact_luts(&removed, &mut prefix_sum);

        self.scratch.removed = removed;
        self.scratch.prefix_sum = prefix_sum;
        self.has_changed = true;
    }

    /// Close the gaps left by `removed` in the packed buffer and rewrite the
    /// offsets of the entries that survived.
    ///
    /// `removed` must be sorted by `lut_start`.
    fn compact_luts(&mut self, removed: &[CachedRamp], prefix_sum: &mut Vec<u32>) {
        if removed.is_empty() {
            return;
        }

        // `prefix_sum[i]` is the total texel width removed before `removed[i]`, so
        // a survivor's offset shrinks by the entry matching its position in
        // `removed`. The leading zero makes the partition point below index it
        // directly.
        prefix_sum.clear();
        prefix_sum.push(0);

        let mut write_pos = 0;
        let mut read_pos = 0;

        for ramp in removed {
            let remove_start = (ramp.lut_start * BYTES_PER_TEXEL) as usize;
            let remove_end = remove_start + (ramp.width * BYTES_PER_TEXEL) as usize;

            if read_pos < remove_start {
                self.luts.copy_within(read_pos..remove_start, write_pos);
                write_pos += remove_start - read_pos;
            }

            read_pos = remove_end;
            prefix_sum.push(prefix_sum.last().copied().unwrap_or(0) + ramp.width);
        }

        let luts_len = self.luts.len();
        if read_pos < luts_len {
            self.luts.copy_within(read_pos..luts_len, write_pos);
            write_pos += luts_len - read_pos;
        }
        self.luts.truncate(write_pos);

        for entry in self.entries.values_mut() {
            let pos = removed.partition_point(|ramp| ramp.lut_start < entry.ramp.lut_start);
            entry.ramp.lut_start -= prefix_sum[pos];
        }
    }
}

/// The packed LUT bytes padded to a texture's footprint for one upload.
///
/// Dereferences to the bytes a texel copy consumes. Dropping it trims the padding
/// and hands the buffer back to the cache, so the cache is only ever borrowed for
/// the duration of the upload.
#[derive(Debug)]
pub struct LutUpload<'a> {
    cache: &'a mut GradientCache,
    bytes: Vec<u8>,
    logical_len: usize,
    layout: GradientTextureLayout,
}

impl LutUpload<'_> {
    /// The texture geometry these bytes were padded for.
    pub fn layout(&self) -> GradientTextureLayout {
        self.layout
    }

    /// Row stride in bytes for the texel copy.
    pub fn bytes_per_row(&self) -> u32 {
        self.layout.bytes_per_row()
    }

    /// Number of bytes that hold actual ramp data; the rest is zero padding.
    pub fn logical_len(&self) -> usize {
        self.logical_len
    }
}

impl std::ops::Deref for LutUpload<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl Drop for LutUpload<'_> {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.bytes);
        bytes.truncate(self.logical_len);
        self.cache.restore_luts(bytes);
    }
}

/// Bake `gradient`'s colour ramp, append it to `output`, and return its texel width.
#[inline(always)]
fn bake_ramp<S: Simd>(simd: S, gradient: &EncodedGradient, output: &mut Vec<u8>) -> usize {
    let lut = gradient.u8_lut(simd);
    let bytes: &[u8] = bytemuck::cast_slice(lut.lut());
    output.extend_from_slice(bytes);
    lut.width()
}
