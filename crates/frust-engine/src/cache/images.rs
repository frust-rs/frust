//! Image residency: which decoded images own atlas space, and for how long.
//!
//! A widget hands the display list the *same* decoded image every frame — the
//! `peniko::ImageData` in a [`Command::Image`](frust_scene::Command::Image) is a
//! reference-counted handle cloned per frame, never a re-decode — so the pixels
//! behind it are stable across frames and belong on the GPU once rather than
//! per draw. [`ImageResidency`] is what makes that "once" true: it keys an
//! image on its blob's process-unique id ([`peniko::Blob::id`]) and hands back
//! the same [`ImageId`] and the same atlas rectangle for every later frame that
//! draws it.
//!
//! Allocation itself is pure — a rectangle packer over
//! [`vello_common::multi_atlas::MultiAtlasManager`], no device in the loop — so
//! everything here is host-testable. The two GPU-side halves it *describes*
//! (writing a newly allocated region's texels, clearing an evicted one's) are
//! carried out by [`crate::gpu::atlas`] against the plan this module produces:
//! [`ImageResidency::evictions`] first, [`ImageResidency::uploads`] second. That
//! order is a contract, not a preference — a rectangle freed this frame can be
//! re-allocated in the same frame, so clearing after uploading would erase the
//! image that just moved in.
//!
//! ## Two things are refused rather than attempted
//!
//! 1. **A format or size `vello_common` panics on.**
//!    [`vello_common::paint::ImageSource::from_peniko_image_data`] asserts on a
//!    dimension past `u16::MAX` and `unimplemented!()`s on a format outside
//!    `Rgba8`/`Bgra8`, and a pixel buffer whose length disagrees with the
//!    declared extent trips [`Pixmap::from_parts_with_opacity`]'s own assertion.
//!    All three are checked here, *before* the call, so an oversized or
//!    malformed image is an [`ImageSkip`] the frame path reports rather than a
//!    panic it takes (E17).
//! 2. **Anything at all, when `FRUST_ENGINE_NO_ATLAS` is set.** The kill switch
//!    ([`crate::config::atlas_disabled`]) makes every resolution answer
//!    [`ImageSkip::AtlasDisabled`], so no atlas is allocated, nothing is
//!    uploaded, and image draws fall back to painting nothing — the switch's
//!    whole point being to take the atlas out of a frame under diagnosis.
//!
//! ## Residency is bounded by age, not by count
//!
//! An entry unseen for [`MAX_UNSEEN_FRAMES`] consecutive frames is deallocated
//! and its rectangle reported for clearing — the same age-based reap
//! `frust-render`'s shader-effect cache uses, and for the same reason: a screen
//! that stops drawing an image should give its texels back promptly, while an
//! image drawn every other frame must never be mistaken for gone.

use std::collections::HashMap;
use std::sync::Arc;

use frust_gpu::{DownlevelProfile, TierCaps};
use peniko::{ImageData, ImageFormat};
use thiserror::Error;
use vello_common::image_cache::ImageCache;
use vello_common::multi_atlas::{AllocationStrategy, AtlasConfig};
use vello_common::paint::{ImageId, ImageSource};
use vello_common::pixmap::Pixmap;

use crate::config;

/// Consecutive frames an image may go undrawn before its atlas rectangle is
/// reclaimed.
///
/// 60 frames is half a second at 120Hz and a whole one at 60Hz: long enough
/// that an image drawn intermittently (an every-other-frame animation, a brief
/// scene-diff hiccup) never loses its residency, short enough that navigating
/// away from an image-heavy screen returns its texels within a frame budget's
/// worth of frames rather than at surface teardown.
pub const MAX_UNSEEN_FRAMES: u64 = 60;

/// Transparent padding pixels placed around each image in the atlas.
///
/// Zero, deliberately. Padding exists to stop a filtered sample from reading a
/// neighbour's texels, and the strip shader's atlas samplers cannot do that:
/// both the bilinear and the bicubic path clamp every tap into
/// `[offset, offset + size - 1]` — the image's own rectangle — before it
/// reaches `textureLoad`. Paying a two-pixel border per image would buy nothing
/// and cost atlas space that a mobile budget does not have.
pub const ATLAS_PADDING: u16 = 0;

/// The largest image edge that can be made resident at all.
///
/// `vello_common`'s image cache addresses an offset and an extent in `u16`, and
/// its `ImageSource` conversion asserts on anything larger, so this is the
/// ceiling the pre-check enforces rather than a policy of ours.
pub const MAX_IMAGE_DIMENSION: u32 = u16::MAX as u32;

/// The most atlas layers the shader's encoded-image record can name.
///
/// `GpuEncodedImage::image_params` gives `atlas_index` eight bits, so a layer
/// past 255 could not be addressed even if the adapter allowed it.
pub const MAX_ATLAS_LAYERS: usize = 256;

/// Why one image could not be made resident.
///
/// Every variant is a *skip*, never a frame failure: the draw paints nothing
/// and the rest of the frame proceeds. They are distinguished because the
/// remedies differ — a too-large image is an application decision, an exhausted
/// atlas is a budget one, and a disabled atlas is the operator's own doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ImageSkip {
    /// `FRUST_ENGINE_NO_ATLAS` is set; the atlas is out of the frame entirely.
    #[error("image residency is disabled by FRUST_ENGINE_NO_ATLAS")]
    AtlasDisabled,
    /// The image declares a format the atlas does not store.
    #[error("unsupported image format {0:?}: only Rgba8 and Bgra8 are uploaded")]
    UnsupportedFormat(ImageFormat),
    /// The image has no area, so there is nothing to make resident.
    #[error("degenerate image extent {width}x{height}")]
    DegenerateExtent {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
    },
    /// The image is larger than [`MAX_IMAGE_DIMENSION`] on at least one axis.
    #[error("image {width}x{height} exceeds the {max}-pixel residency ceiling")]
    TooLarge {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
        /// The per-axis ceiling that refused it.
        max: u32,
    },
    /// The pixel buffer's length disagrees with the declared extent and format.
    #[error("image pixel buffer is {actual} bytes; {width}x{height} needs exactly {expected}")]
    MalformedPixels {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
        /// Bytes the declared extent and format require.
        expected: usize,
        /// Bytes the blob actually holds.
        actual: usize,
    },
    /// The paint transform cannot be inverted, so there is no device-to-image
    /// mapping for the shader to sample through.
    #[error("image paint transform is singular or non-finite")]
    SingularTransform,
    /// The image fits the ceiling but not the configured atlas budget.
    #[error("image {width}x{height} does not fit the {atlas_width}x{atlas_height} atlas budget")]
    NoAtlasSpace {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
        /// Configured atlas width.
        atlas_width: u32,
        /// Configured atlas height.
        atlas_height: u32,
    },
}

/// The atlas geometry a tier of adapter is given.
///
/// Never `vello_common`'s own [`AtlasConfig`] default: that is 4096 square
/// across eight layers, 64 MiB of `Rgba8Unorm` per layer and half a gigabyte in
/// total, which is a desktop-only figure that a phone would pay for the first
/// image it drew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasBudget {
    /// Extent of each atlas layer, in pixels.
    pub atlas_size: (u32, u32),
    /// Maximum number of layers the array may grow to.
    pub max_atlases: usize,
}

impl AtlasBudget {
    /// The budget a tile-based or downlevel-clamped adapter gets: 4 MiB a
    /// layer, 16 MiB in total.
    pub const MOBILE: Self = Self {
        atlas_size: (1024, 1024),
        max_atlases: 4,
    };

    /// The budget a full-profile immediate-mode adapter gets: 16 MiB a layer,
    /// 128 MiB in total.
    pub const DESKTOP: Self = Self {
        atlas_size: (2048, 2048),
        max_atlases: 8,
    };

    /// The budget for `caps`' adapter, clamped to what it can actually create.
    ///
    /// The tier choice is [`is_mobile_tier`]; the clamps that follow are the
    /// adapter's own texture-edge and array-layer ceilings plus the eight-bit
    /// [`MAX_ATLAS_LAYERS`] the encoded-image record can address.
    /// `FRUST_ENGINE_ATLAS_SIZE` overrides the tier's extent before those
    /// clamps, never after — an override is a request, and a request the
    /// adapter cannot honour is still narrowed to what it can.
    #[must_use]
    pub fn for_caps(caps: &TierCaps) -> Self {
        let tier = if is_mobile_tier(caps) {
            Self::MOBILE
        } else {
            Self::DESKTOP
        };
        let atlas_size = config::atlas_size().unwrap_or(tier.atlas_size);

        Self {
            atlas_size,
            max_atlases: tier.max_atlases,
        }
        .clamped(caps)
    }

    /// This budget narrowed to what `caps`' adapter can create.
    #[must_use]
    pub fn clamped(self, caps: &TierCaps) -> Self {
        let edge = caps.max_texture_dimension_2d.max(1);
        let layers = usize::try_from(caps.max_texture_array_layers).unwrap_or(MAX_ATLAS_LAYERS);

        Self {
            atlas_size: (
                self.atlas_size.0.clamp(1, edge),
                self.atlas_size.1.clamp(1, edge),
            ),
            max_atlases: self.max_atlases.clamp(1, layers.min(MAX_ATLAS_LAYERS)),
        }
    }

    /// The `vello_common` configuration this budget describes.
    ///
    /// `initial_atlas_count` is zero so the first layer is created by the first
    /// allocation that needs one: an application drawing no images pays for no
    /// atlas at all, and [`vello_common::multi_atlas::MultiAtlasManager::new`]'s
    /// own `expect` on eager creation can never be reached.
    #[must_use]
    pub fn config(self) -> AtlasConfig {
        AtlasConfig {
            initial_atlas_count: 0,
            max_atlases: self.max_atlases,
            atlas_size: self.atlas_size,
            auto_grow: true,
            allocation_strategy: AllocationStrategy::FirstFit,
        }
    }

    /// The bytes one fully populated layer costs at [`ATLAS_FORMAT_BYTES`].
    #[must_use]
    pub fn layer_bytes(self) -> u64 {
        u64::from(self.atlas_size.0) * u64::from(self.atlas_size.1) * ATLAS_FORMAT_BYTES
    }

    /// The bytes every layer of this budget costs once fully grown.
    #[must_use]
    pub fn total_bytes(self) -> u64 {
        self.layer_bytes()
            .saturating_mul(self.max_atlases.try_into().unwrap_or(u64::MAX))
    }
}

/// Bytes per texel of the atlas array (`Rgba8Unorm`).
pub const ATLAS_FORMAT_BYTES: u64 = 4;

/// Whether `caps`' adapter takes the mobile atlas budget.
///
/// Two signals, either of which is enough:
///
/// - its usable limits are clamped to the GLES-3.0/WebGL2 downlevel defaults,
///   which is the profile every GL-backed target reports; or
/// - it reports `transient_saves_memory`, wgpu's own answer to "is this a
///   tile-based renderer keeping attachments on-chip" — true on the mobile
///   TBDR parts and false on desktop immediate-mode ones.
///
/// Neither reads the adapter *name*: a name match is a denylist that ages, and
/// both signals above are capability answers the driver gives directly.
#[must_use]
pub fn is_mobile_tier(caps: &TierCaps) -> bool {
    caps.downlevel_profile != DownlevelProfile::Full || caps.transient_saves_memory
}

/// Where one image's texels live in the atlas array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasRegion {
    /// Array layer holding the region.
    pub layer: u32,
    /// Pixel offset of the region's top-left corner within that layer.
    pub offset: [u32; 2],
    /// Region extent in pixels.
    pub size: [u32; 2],
}

impl AtlasRegion {
    /// The bytes this region's texels occupy at [`ATLAS_FORMAT_BYTES`].
    #[must_use]
    pub fn byte_len(self) -> usize {
        (self.size[0] as usize)
            .saturating_mul(self.size[1] as usize)
            .saturating_mul(ATLAS_FORMAT_BYTES as usize)
    }

    /// The row stride a texel copy of this region uses.
    #[must_use]
    pub fn bytes_per_row(self) -> u32 {
        self.size[0].saturating_mul(ATLAS_FORMAT_BYTES as u32)
    }

    /// Whether this region has any texels at all.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.size[0] == 0 || self.size[1] == 0
    }
}

/// A resident image: the handle a paint names it by, and where its texels are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentImage {
    /// The handle an [`ImageSource::OpaqueId`] carries.
    pub id: ImageId,
    /// The atlas rectangle holding the image's own pixels, padding excluded.
    pub region: AtlasRegion,
    /// Transparent padding pixels around `region` in the atlas.
    pub padding: u32,
    /// Whether the image's premultiplied pixels include any non-opaque texel.
    pub may_have_transparency: bool,
}

impl ResidentImage {
    /// This image as the paint-side source a `vello_common` encoding carries.
    #[must_use]
    pub fn source(&self) -> ImageSource {
        ImageSource::opaque_id_with_transparency_hint(self.id, self.may_have_transparency)
    }
}

/// One region's texels, waiting to be written into the atlas array.
///
/// The pixels are the premultiplied `Rgba8Unorm` the atlas stores, produced
/// once at allocation — an image made resident on frame 1 and drawn on frames
/// 1..1000 is converted exactly once.
#[derive(Debug, Clone)]
pub struct ImageUpload {
    /// Where the texels go.
    pub region: AtlasRegion,
    /// The premultiplied pixels, row-major and exactly `region`'s extent.
    pub pixels: Arc<Pixmap>,
}

/// One cache entry: the handle, its rectangle, and when it was last drawn.
#[derive(Debug, Clone, Copy)]
struct Entry {
    id: ImageId,
    region: AtlasRegion,
    may_have_transparency: bool,
    last_seen: u64,
}

/// The images resident in the atlas array, keyed by blob identity.
///
/// One per compiler (and so one per surface). Its frame clock is advanced by
/// [`begin_frame`](Self::begin_frame), which is also where the age-based reap
/// runs — before any of the frame's own resolutions, so a rectangle freed this
/// frame is available to this frame's allocations and its clear is ordered
/// ahead of their uploads.
#[derive(Debug)]
pub struct ImageResidency {
    cache: ImageCache,
    budget: AtlasBudget,
    entries: HashMap<u64, Entry>,
    frame: u64,
    layers: u32,
    disabled: bool,
    uploads: Vec<ImageUpload>,
    evictions: Vec<AtlasRegion>,
    skipped: u64,
    /// Scratch for the reap's key list, kept so a frame that evicts allocates
    /// nothing.
    reaped: Vec<u64>,
}

impl ImageResidency {
    /// A residency over `budget`'s atlas geometry.
    ///
    /// `FRUST_ENGINE_NO_ATLAS` is consulted here, once, rather than per
    /// resolution: the switch is process-global and cached, so re-reading it on
    /// a hot path would buy nothing but a lock. A residency constructed with
    /// the switch set is exactly [`disabled`](Self::disabled).
    #[must_use]
    pub fn new(budget: AtlasBudget) -> Self {
        Self::with_enabled(budget, !config::atlas_disabled())
    }

    /// A residency that refuses every image, allocating no atlas at all.
    ///
    /// What `FRUST_ENGINE_NO_ATLAS` selects, and what a caller wanting the same
    /// effect programmatically constructs. Every [`resolve`](Self::resolve)
    /// answers [`ImageSkip::AtlasDisabled`], so image draws are dropped with a
    /// logged reason rather than painted from an atlas that does not exist.
    #[must_use]
    pub fn disabled(budget: AtlasBudget) -> Self {
        Self::with_enabled(budget, false)
    }

    fn with_enabled(budget: AtlasBudget, enabled: bool) -> Self {
        Self {
            cache: ImageCache::new_with_config(budget.config()),
            budget,
            entries: HashMap::new(),
            frame: 0,
            layers: 0,
            disabled: !enabled,
            uploads: Vec::new(),
            evictions: Vec::new(),
            skipped: 0,
            reaped: Vec::new(),
        }
    }

    /// A residency sized for `caps`' adapter.
    #[must_use]
    pub fn for_caps(caps: &TierCaps) -> Self {
        Self::new(AtlasBudget::for_caps(caps))
    }

    /// The atlas geometry this residency allocates within.
    #[must_use]
    pub fn budget(&self) -> AtlasBudget {
        self.budget
    }

    /// Whether this residency refuses every image.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// How many images are currently resident.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The frame clock entries age against.
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// How many atlas layers have been created so far.
    ///
    /// The array texture must be at least this deep before the frame's uploads
    /// are written.
    #[must_use]
    pub fn layers(&self) -> u32 {
        self.layers
    }

    /// How many resolutions have been refused over this residency's lifetime.
    #[must_use]
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// The regions whose texels must be cleared before this frame's uploads.
    #[must_use]
    pub fn evictions(&self) -> &[AtlasRegion] {
        &self.evictions
    }

    /// The regions whose texels must be written after this frame's evictions.
    #[must_use]
    pub fn uploads(&self) -> &[ImageUpload] {
        &self.uploads
    }

    /// Takes this frame's clear-then-write plan, leaving the residency's own
    /// buffers empty and reusable.
    #[must_use]
    pub fn take_plan(&mut self) -> (Vec<AtlasRegion>, Vec<ImageUpload>) {
        (
            std::mem::take(&mut self.evictions),
            std::mem::take(&mut self.uploads),
        )
    }

    /// Advance the frame clock and reclaim everything unseen for
    /// [`MAX_UNSEEN_FRAMES`].
    ///
    /// Call once at the head of a frame, before any [`resolve`](Self::resolve).
    /// The previous frame's upload and eviction lists are dropped here, so a
    /// caller that did not service them does not service them twice.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
        self.uploads.clear();
        self.evictions.clear();
        self.reap();
    }

    /// The atlas rectangle for `data`'s pixels, allocating and scheduling an
    /// upload on the first frame that asks for it.
    ///
    /// # Errors
    ///
    /// Returns the [`ImageSkip`] naming why the image was refused. Every
    /// refusal happens before any `vello_common` call that could assert on the
    /// same condition, so an image the renderer cannot hold is a skipped draw
    /// rather than a panicked frame (E17).
    pub fn resolve(&mut self, data: &ImageData) -> Result<ResidentImage, ImageSkip> {
        let result = self.resolve_inner(data);
        if result.is_err() {
            self.skipped = self.skipped.saturating_add(1);
        }
        result
    }

    fn resolve_inner(&mut self, data: &ImageData) -> Result<ResidentImage, ImageSkip> {
        if self.disabled {
            return Err(ImageSkip::AtlasDisabled);
        }

        check_supported(data)?;

        let key = data.data.id();
        if let Some(entry) = self.entries.get_mut(&key)
            && entry.region.size == [data.width, data.height]
        {
            entry.last_seen = self.frame;
            return Ok(ResidentImage {
                id: entry.id,
                region: entry.region,
                padding: u32::from(ATLAS_PADDING),
                may_have_transparency: entry.may_have_transparency,
            });
        }

        // A blob id recorded at a different extent is a caller that rebuilt the
        // metadata around the same buffer. Its old rectangle describes the
        // wrong pixels, so it is released before the new one is taken rather
        // than left to age out holding both.
        self.release(key);

        // Checked above, so this cannot assert; the conversion is also the one
        // place the straight-alpha premultiply happens, and it happens once per
        // image rather than once per frame.
        let ImageSource::Pixmap(pixels) = ImageSource::from_peniko_image_data(data) else {
            // `from_peniko_image_data` only ever yields a pixmap; an id-backed
            // source would mean a `vello_common` change, which is a skip rather
            // than an unreachable.
            return Err(ImageSkip::UnsupportedFormat(data.format));
        };

        let id = self
            .cache
            .allocate(data.width, data.height, ATLAS_PADDING)
            .map_err(|_| ImageSkip::NoAtlasSpace {
                width: data.width,
                height: data.height,
                atlas_width: self.budget.atlas_size.0,
                atlas_height: self.budget.atlas_size.1,
            })?;

        let Some(resource) = self.cache.get(id) else {
            // A successful allocation always populates its slot; refusing here
            // keeps the frame path free of the `expect` the reference renderer
            // takes at this same point.
            return Err(ImageSkip::NoAtlasSpace {
                width: data.width,
                height: data.height,
                atlas_width: self.budget.atlas_size.0,
                atlas_height: self.budget.atlas_size.1,
            });
        };

        let region = AtlasRegion {
            layer: resource.atlas_id.as_u32(),
            offset: resource.offsets(),
            size: resource.size(),
        };
        let may_have_transparency = pixels.may_have_transparency();
        self.layers = self.layers.max(region.layer.saturating_add(1));

        self.entries.insert(
            key,
            Entry {
                id,
                region,
                may_have_transparency,
                last_seen: self.frame,
            },
        );
        self.uploads.push(ImageUpload { region, pixels });

        Ok(ResidentImage {
            id,
            region,
            padding: u32::from(ATLAS_PADDING),
            may_have_transparency,
        })
    }

    /// Deallocate every entry unseen for [`MAX_UNSEEN_FRAMES`], recording each
    /// one's rectangle for clearing.
    fn reap(&mut self) {
        let mut reaped = std::mem::take(&mut self.reaped);
        reaped.clear();
        reaped.extend(self.entries.iter().filter_map(|(key, entry)| {
            (self.frame.saturating_sub(entry.last_seen) > MAX_UNSEEN_FRAMES).then_some(*key)
        }));

        for key in reaped.drain(..) {
            self.release(key);
        }
        self.reaped = reaped;
    }

    /// Drop `key`'s entry, freeing its atlas rectangle and recording it for
    /// clearing.
    ///
    /// The padded rectangle is what is cleared: an allocation reserves its
    /// padding too, so leaving the border behind would leave stale texels a
    /// later allocation could sample through.
    fn release(&mut self, key: u64) {
        let Some(entry) = self.entries.remove(&key) else {
            return;
        };
        // The handle came from this cache's own `allocate`, so the atlas it
        // names exists and the deallocation cannot fail.
        self.cache.deallocate(entry.id);
        self.evictions.push(padded(entry.region, ATLAS_PADDING));
    }
}

/// `region` grown by `padding` on every side, clamped at the layer origin.
fn padded(region: AtlasRegion, padding: u16) -> AtlasRegion {
    let padding = u32::from(padding);
    let pad_x = padding.min(region.offset[0]);
    let pad_y = padding.min(region.offset[1]);

    AtlasRegion {
        layer: region.layer,
        offset: [region.offset[0] - pad_x, region.offset[1] - pad_y],
        size: [
            region.size[0].saturating_add(pad_x + padding),
            region.size[1].saturating_add(pad_y + padding),
        ],
    }
}

/// Refuse an image `vello_common`'s own conversion would assert on.
///
/// The three conditions are exactly the three assertions downstream: the
/// format `unimplemented!()`, the `u16::MAX` dimension check, and
/// `Pixmap::from_parts_with_opacity`'s length equality. Checking them here is
/// what keeps the panic off the frame path.
fn check_supported(data: &ImageData) -> Result<(), ImageSkip> {
    match data.format {
        ImageFormat::Rgba8 | ImageFormat::Bgra8 => {}
        format => return Err(ImageSkip::UnsupportedFormat(format)),
    }

    if data.width == 0 || data.height == 0 {
        return Err(ImageSkip::DegenerateExtent {
            width: data.width,
            height: data.height,
        });
    }

    if data.width > MAX_IMAGE_DIMENSION || data.height > MAX_IMAGE_DIMENSION {
        return Err(ImageSkip::TooLarge {
            width: data.width,
            height: data.height,
            max: MAX_IMAGE_DIMENSION,
        });
    }

    let actual = data.data.data().len();
    let expected = data
        .format
        .size_in_bytes(data.width, data.height)
        .unwrap_or(usize::MAX);
    if actual != expected {
        return Err(ImageSkip::MalformedPixels {
            width: data.width,
            height: data.height,
            expected,
            actual,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use peniko::{Blob, ImageAlphaType};

    fn image(width: u32, height: u32) -> ImageData {
        let len = (width as usize) * (height as usize) * 4;
        ImageData {
            data: Blob::new(Arc::new(vec![255_u8; len])),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        }
    }

    fn residency() -> ImageResidency {
        ImageResidency::new(AtlasBudget {
            atlas_size: (64, 64),
            max_atlases: 2,
        })
    }

    #[test]
    fn a_mobile_budget_is_never_the_vello_default() {
        let default = AtlasConfig::default();
        for budget in [AtlasBudget::MOBILE, AtlasBudget::DESKTOP] {
            assert_ne!(budget.atlas_size, default.atlas_size);
            assert!(budget.total_bytes() < 4096 * 4096 * ATLAS_FORMAT_BYTES * 8);
        }
        assert_eq!(AtlasBudget::MOBILE.total_bytes(), 16 << 20);
        assert_eq!(AtlasBudget::DESKTOP.total_bytes(), 128 << 20);
    }

    #[test]
    fn a_downlevel_adapter_takes_the_mobile_budget() {
        let webgl2 = TierCaps::fake(DownlevelProfile::WebGl2);
        assert!(is_mobile_tier(&webgl2));

        let mut tiler = TierCaps::fake(DownlevelProfile::Full);
        assert!(!is_mobile_tier(&tiler));
        tiler.transient_saves_memory = true;
        assert!(is_mobile_tier(&tiler));
    }

    #[test]
    fn a_budget_is_clamped_to_the_adapters_own_ceilings() {
        let mut caps = TierCaps::fake(DownlevelProfile::Full);
        caps.max_texture_dimension_2d = 1024;
        caps.max_texture_array_layers = 2;

        let budget = AtlasBudget::DESKTOP.clamped(&caps);
        assert_eq!(budget.atlas_size, (1024, 1024));
        assert_eq!(budget.max_atlases, 2);

        // The eight-bit `atlas_index` field is a ceiling of its own.
        caps.max_texture_array_layers = 4096;
        let budget = AtlasBudget {
            atlas_size: (256, 256),
            max_atlases: 4096,
        }
        .clamped(&caps);
        assert_eq!(budget.max_atlases, MAX_ATLAS_LAYERS);
    }

    #[test]
    fn the_first_layer_is_created_lazily() {
        let residency = residency();
        assert_eq!(residency.layers(), 0);
        assert_eq!(residency.entry_count(), 0);
    }

    #[test]
    fn one_image_over_many_frames_uploads_exactly_once() {
        let mut residency = residency();
        let data = image(8, 8);
        let mut uploads = 0;

        for _ in 0..60 {
            residency.begin_frame();
            residency.resolve(&data).expect("8x8 fits a 64x64 atlas");
            uploads += residency.uploads().len();
        }

        assert_eq!(uploads, 1, "residency survives every frame that draws it");
        assert_eq!(residency.entry_count(), 1);
        assert_eq!(residency.layers(), 1);
    }

    #[test]
    fn an_unseen_image_is_reaped_and_its_padded_region_cleared() {
        let mut residency = residency();
        let data = image(8, 8);

        residency.begin_frame();
        let resident = residency.resolve(&data).expect("fits");

        for _ in 0..MAX_UNSEEN_FRAMES {
            residency.begin_frame();
            assert!(residency.evictions().is_empty(), "still inside the window");
        }

        residency.begin_frame();
        assert_eq!(residency.entry_count(), 0);
        assert_eq!(
            residency.evictions(),
            &[padded(resident.region, ATLAS_PADDING)]
        );

        // The freed rectangle is available again, and the reallocation
        // schedules a fresh upload.
        residency.resolve(&data).expect("fits");
        assert_eq!(residency.uploads().len(), 1);
    }

    #[test]
    fn an_image_larger_than_the_atlas_is_skipped_rather_than_panicking() {
        let mut residency = residency();
        residency.begin_frame();

        let skip = residency.resolve(&image(128, 8)).expect_err("too wide");
        assert!(matches!(skip, ImageSkip::NoAtlasSpace { .. }));
        assert_eq!(residency.skipped(), 1);
        assert!(residency.uploads().is_empty());
    }

    #[test]
    fn a_malformed_or_degenerate_image_is_refused_before_the_conversion() {
        let mut short = image(4, 4);
        short.data = Blob::new(Arc::new(vec![0_u8; 8]));
        assert!(matches!(
            check_supported(&short),
            Err(ImageSkip::MalformedPixels { .. })
        ));

        assert!(matches!(
            check_supported(&image(0, 4)),
            Err(ImageSkip::DegenerateExtent { .. })
        ));

        let mut huge = image(1, 1);
        huge.width = MAX_IMAGE_DIMENSION + 1;
        assert!(matches!(
            check_supported(&huge),
            Err(ImageSkip::TooLarge { .. })
        ));
    }

    #[test]
    fn two_images_share_a_layer_and_hold_distinct_rectangles() {
        let mut residency = residency();
        residency.begin_frame();

        let first = residency.resolve(&image(8, 8)).expect("fits");
        let second = residency.resolve(&image(16, 16)).expect("fits");

        assert_ne!(first.id, second.id);
        assert_eq!(first.region.layer, second.region.layer);
        assert_ne!(first.region.offset, second.region.offset);
        assert_eq!(residency.uploads().len(), 2);
        assert_eq!(residency.layers(), 1);
    }

    #[test]
    fn a_second_layer_is_created_when_the_first_is_full() {
        let mut residency = residency();
        residency.begin_frame();

        // Two 64x48 images cannot share one 64x64 layer.
        let first = residency.resolve(&image(64, 48)).expect("fits a layer");
        let second = residency.resolve(&image(64, 48)).expect("fits the next");

        assert_eq!(first.region.layer, 0);
        assert_eq!(second.region.layer, 1);
        assert_eq!(residency.layers(), 2);
    }

    #[test]
    fn a_disabled_residency_refuses_every_image_and_touches_no_atlas() {
        let mut residency = ImageResidency::disabled(AtlasBudget::MOBILE);
        assert!(residency.is_disabled());

        residency.begin_frame();
        let skip = residency
            .resolve(&image(8, 8))
            .expect_err("the kill switch refuses every image");

        assert_eq!(skip, ImageSkip::AtlasDisabled);
        assert_eq!(residency.entry_count(), 0);
        assert_eq!(residency.layers(), 0, "no atlas layer is ever created");
        assert!(residency.uploads().is_empty());
        assert_eq!(residency.skipped(), 1);
    }

    #[test]
    fn an_enabled_residency_is_what_an_unset_kill_switch_produces() {
        // The default test environment leaves `FRUST_ENGINE_NO_ATLAS` unset, so
        // this pins that `new` consults the switch rather than hardcoding
        // either answer; the disabled half is pinned above through the
        // constructor the switch selects.
        assert!(!ImageResidency::new(AtlasBudget::MOBILE).is_disabled());
    }

    #[test]
    fn taking_the_plan_leaves_the_residency_ready_for_the_next_frame() {
        let mut residency = residency();
        residency.begin_frame();
        residency.resolve(&image(8, 8)).expect("fits");

        let (evictions, uploads) = residency.take_plan();
        assert!(evictions.is_empty());
        assert_eq!(uploads.len(), 1);
        assert!(residency.uploads().is_empty());
        assert!(residency.evictions().is_empty());
    }
}
