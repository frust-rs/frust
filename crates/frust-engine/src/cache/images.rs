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
//! ## Residency is committed only once the plan is serviced
//!
//! The cache decides residency long before anything reaches a queue: a
//! compiled frame can still be refused afterwards (a layer shape the scheduler
//! will not serve, a page past the pool's ceiling, coverage or paints outgrowing
//! their textures), and a frame refused after compiling is never drawn and never
//! uploaded. So the plan is *pending* rather than taken: [`ImageResidency::plan`]
//! hands out a copy, [`begin_frame`](ImageResidency::begin_frame) leaves it
//! alone, and only [`acknowledge_plan`](ImageResidency::acknowledge_plan) —
//! called by a consumer that has actually written the regions into a live atlas
//! — clears it.
//!
//! That is what makes the entry map and the atlas's real contents agree. Without
//! it, a refused frame leaves an image *recorded as resident* whose texels were
//! never written: every later frame resolves the entry, schedules no upload
//! (residency is a hit), and either drops the draw for the life of the process
//! or samples a rectangle nothing ever wrote. Re-emitting the same plan until it
//! is acknowledged costs a vector copy per frame in the steady state — where the
//! plan is empty — and is what makes "an image drawn on a thousand frames
//! uploads once" true of the *atlas* rather than only of the cache.
//!
//! ## An image larger than the atlas is minified, not dropped
//!
//! A source whose own extent will not fit one atlas layer is downsampled to fit
//! ([`fit_extent`], [`minify`]) instead of being refused. A photograph decoded
//! at its capture resolution is routinely several times the atlas budget while
//! its destination rectangle is a thumbnail, so refusing it drops a draw the
//! display list plainly describes; and every texel past the destination's own
//! scale is one the sampler would have thrown away anyway.
//!
//! The filter is a plain box average over the premultiplied texels, hand-written
//! rather than pulled in: the workspace's version pins are law, an image decoder
//! is not a dependency this crate carries, and a box average is exactly right
//! for the minification direction (every output texel is the mean of the source
//! texels it covers). The aspect ratio is preserved, so the paint transform's
//! two axes stay in step; what changes is the resident rectangle's extent, which
//! [`ResidentImage::natural`] records alongside it so the consumer can rescale
//! the encoded paint's own transform into the smaller rectangle.
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
//!    panic it takes (E17). These, plus an atlas with no room left in it, are
//!    now the only things [`ImageResidency::skipped`] counts: an image that is
//!    merely *big* is minified and drawn.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use frust_gpu::{DownlevelProfile, TierCaps};
use peniko::color::PremulRgba8;
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
    ///
    /// `FRUST_ENGINE_ATLAS_SIZE` overrides the tier's extent, and the override
    /// is **budgeted, not unbudgeted**: it passes through
    /// [`within_total_bytes`](Self::within_total_bytes) against the tier's own
    /// [`total_bytes`](Self::total_bytes) before the adapter clamps. So the knob
    /// redistributes a tier's memory between extent and depth — one large layer
    /// instead of eight small ones — and cannot spend more of it than the tier
    /// itself would. Clamped to the adapter last, because an override is a
    /// request and a request the adapter cannot honour is still narrowed to what
    /// it can.
    #[must_use]
    pub fn for_caps(caps: &TierCaps) -> Self {
        let tier = if is_mobile_tier(caps) {
            Self::MOBILE
        } else {
            Self::DESKTOP
        };
        let Some(atlas_size) = config::atlas_size() else {
            return tier.clamped(caps);
        };

        Self {
            atlas_size,
            max_atlases: tier.max_atlases,
        }
        .within_total_bytes(tier.total_bytes())
        .clamped(caps)
    }

    /// This budget narrowed so a fully grown array costs at most `total_bytes`.
    ///
    /// The extent goes first, scaled uniformly until one layer fits inside the
    /// whole allowance; the layer count then takes whatever multiple of that
    /// layer is left, never below one. Scaling the extent rather than only the
    /// depth is what makes the ceiling hold at all: a single 16384-square layer
    /// is a gigabyte on its own, so a clamp that could only reduce the layer
    /// *count* would have nothing left to reduce.
    ///
    /// Both axes are floored after the uniform scale, which can only take the
    /// product below the allowance — except where an extreme aspect ratio floors
    /// one axis to nothing and it is raised back to a texel, so each axis is
    /// then held against what the other leaves.
    #[must_use]
    pub fn within_total_bytes(self, total_bytes: u64) -> Self {
        // A single texel is the smallest atlas that exists, so an allowance
        // below one is treated as one rather than producing a zero extent the
        // packer could not address.
        let allowance = total_bytes.max(ATLAS_FORMAT_BYTES);
        let max_texels = allowance / ATLAS_FORMAT_BYTES;

        let (width, height) = self.atlas_size;
        let texels = u64::from(width).saturating_mul(u64::from(height));
        let atlas_size = if texels <= max_texels {
            (width, height)
        } else {
            // `as` on a float saturates rather than wrapping, and both factors
            // are finite and positive, so the narrowing is a plain floor.
            let scale = (max_texels as f64 / texels.max(1) as f64).sqrt();
            let scaled_w = ((f64::from(width) * scale) as u32).max(1);
            let scaled_h = ((f64::from(height) * scale) as u32).max(1);
            let fitted_w = fit_axis(scaled_w, max_texels / u64::from(scaled_h));
            let fitted_h = fit_axis(scaled_h, max_texels / u64::from(fitted_w));
            (fitted_w, fitted_h)
        };

        let layer_bytes = u64::from(atlas_size.0)
            .saturating_mul(u64::from(atlas_size.1))
            .saturating_mul(ATLAS_FORMAT_BYTES)
            .max(1);
        let layers = usize::try_from(allowance / layer_bytes).unwrap_or(MAX_ATLAS_LAYERS);

        Self {
            atlas_size,
            max_atlases: self.max_atlases.min(layers).max(1),
        }
    }

    /// Whether `region` lies inside an atlas array of `layers` layers at this
    /// budget's per-layer extent.
    ///
    /// The pure counterpart of [`crate::gpu::atlas::AtlasArray::contains`],
    /// answerable with no device: a consumer that records where an image lives
    /// before the array exists checks the rectangle here, so it can never come
    /// to name a region the array will refuse to write and then sample it as
    /// whatever the texture happened to hold.
    #[must_use]
    pub fn contains(self, region: AtlasRegion, layers: u32) -> bool {
        !region.is_empty()
            && region.layer < layers
            && region.offset[0].saturating_add(region.size[0]) <= self.atlas_size.0
            && region.offset[1].saturating_add(region.size[1]) <= self.atlas_size.1
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
    ///
    /// Equal to [`natural`](Self::natural) unless the source was minified to
    /// fit the atlas, in which case it is the smaller rectangle actually
    /// uploaded.
    pub region: AtlasRegion,
    /// The source's own declared extent, before any minification.
    ///
    /// Carried because it is the extent a paint transform was composed against
    /// — `compile::paint::encode_image_command` derives its natural-to-dest
    /// mapping from the `ImageData`'s declared size, which it reads before this
    /// residency is consulted. A consumer that ends up sampling a *minified*
    /// rectangle has to fold [`minify_scale`](Self::minify_scale) into that
    /// transform, and this is the half of the ratio the atlas rectangle does
    /// not carry.
    pub natural: [u32; 2],
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

    /// The per-axis factor mapping a natural-space texel coordinate onto the
    /// resident rectangle, or `None` when the source was stored at full size.
    ///
    /// `None` rather than `Some((1.0, 1.0))` so a caller can skip the rescale
    /// entirely on the overwhelmingly common path, and so "was this image
    /// minified?" is one question rather than a float comparison.
    #[must_use]
    pub fn minify_scale(&self) -> Option<(f32, f32)> {
        if self.region.size == self.natural {
            return None;
        }
        let natural_w = self.natural[0].max(1) as f32;
        let natural_h = self.natural[1].max(1) as f32;
        Some((
            self.region.size[0] as f32 / natural_w,
            self.region.size[1] as f32 / natural_h,
        ))
    }
}

/// One region's texels, waiting to be written into the atlas array.
///
/// The pixels are the premultiplied `Rgba8Unorm` the atlas stores, produced
/// once at allocation — an image made resident on frame 1 and drawn on frames
/// 1..1000 is converted exactly once.
#[derive(Debug, Clone)]
pub struct ImageUpload {
    /// The handle the paint naming these pixels carries.
    ///
    /// Carried so a consumer rebuilding its own view of residency can key this
    /// upload directly rather than pairing the frame's uploads positionally
    /// against its encoded paints. A plan re-emitted until it is acknowledged
    /// breaks that pairing (an upload can outlive the frame whose draw order
    /// produced it), and an upload that names itself needs no ordering to be
    /// read correctly.
    pub id: ImageId,
    /// Where the texels go.
    pub region: AtlasRegion,
    /// The source's own declared extent, before any minification — see
    /// [`ResidentImage::natural`], whose value this is.
    ///
    /// Carried on the upload because a consumer rebuilding its own view of
    /// residency from a compiled frame has the upload and nothing else, and
    /// `region` alone cannot say whether it holds a minified copy.
    pub natural: [u32; 2],
    /// The premultiplied pixels, row-major and exactly `region`'s extent.
    pub pixels: Arc<Pixmap>,
    /// Whether those pixels include any non-opaque texel — the same hint
    /// [`ResidentImage::may_have_transparency`] carries, so a consumer can
    /// rebuild the whole residency record from this upload alone.
    pub may_have_transparency: bool,
}

/// One cache entry: the handle, its rectangle, and when it was last drawn.
#[derive(Debug, Clone, Copy)]
struct Entry {
    id: ImageId,
    region: AtlasRegion,
    /// The source extent this entry was allocated for, which is what a later
    /// frame's `ImageData` is matched against — `region` may be the minified
    /// rectangle and so says nothing about the source.
    natural: [u32; 2],
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
    minified: u64,
    /// Blob keys whose minification has already been reported, so a source that
    /// is redrawn — or reaped and made resident again — logs once rather than
    /// once per residency.
    reported_minify: HashSet<u64>,
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
            minified: 0,
            reported_minify: HashSet::new(),
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
    ///
    /// Counts only images that could not be uploaded at all — a format or
    /// extent the conversion refuses, a disabled atlas, or an atlas with no
    /// room left. A source merely larger than one atlas layer is minified to
    /// fit and never reaches this counter.
    #[must_use]
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// How many sources have been downsampled to fit the atlas over this
    /// residency's lifetime.
    ///
    /// Observational, and the counter that makes "an image too large for the
    /// atlas is minified, not dropped" measurable rather than asserted.
    #[must_use]
    pub fn minified(&self) -> u64 {
        self.minified
    }

    /// The regions whose texels must be cleared before the pending uploads.
    ///
    /// Pending rather than per-frame: an entry reaped on a frame nobody
    /// serviced is still waiting to be cleared on the next one.
    #[must_use]
    pub fn evictions(&self) -> &[AtlasRegion] {
        &self.evictions
    }

    /// The regions whose texels must be written after the pending evictions.
    ///
    /// Pending rather than per-frame: an image made resident on a frame that was
    /// refused is still waiting to be written on the next one.
    #[must_use]
    pub fn uploads(&self) -> &[ImageUpload] {
        &self.uploads
    }

    /// A copy of the pending clear-then-write plan, evictions first.
    ///
    /// A *copy*, not a drain: the plan stays pending until
    /// [`acknowledge_plan`](Self::acknowledge_plan) says it was serviced against
    /// a live atlas, so a frame refused after compiling re-emits the same plan
    /// on the next frame rather than losing it (see the module doc). The upload
    /// pixels are behind an `Arc`, so a re-emitted plan copies handles rather
    /// than texels, and the steady-state plan is empty on both counts.
    #[must_use]
    pub fn plan(&self) -> (Vec<AtlasRegion>, Vec<ImageUpload>) {
        (self.evictions.clone(), self.uploads.clone())
    }

    /// Whether any part of the pending plan is still unserviced.
    #[must_use]
    pub fn has_pending_plan(&self) -> bool {
        !self.evictions.is_empty() || !self.uploads.is_empty()
    }

    /// Record that the pending plan reached the atlas array, clearing it.
    ///
    /// Call this only once every region it names has actually been written or
    /// cleared. Calling it on a frame that was refused, or whose writes the
    /// array declined, is exactly the defect the pending plan exists to prevent:
    /// the entry map would go on claiming an image is resident whose texels are
    /// whatever the atlas texture happened to hold.
    pub fn acknowledge_plan(&mut self) {
        self.evictions.clear();
        self.uploads.clear();
    }

    /// Advance the frame clock and reclaim everything unseen for
    /// [`MAX_UNSEEN_FRAMES`].
    ///
    /// Call once at the head of a frame, before any [`resolve`](Self::resolve).
    /// An unacknowledged plan deliberately survives this: the frame that was to
    /// service it may have been refused, and dropping it here is what would turn
    /// a refused frame into a permanently unwritten atlas region.
    pub fn begin_frame(&mut self) {
        self.frame = self.frame.saturating_add(1);
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
            && entry.natural == [data.width, data.height]
        {
            entry.last_seen = self.frame;
            return Ok(ResidentImage {
                id: entry.id,
                region: entry.region,
                natural: entry.natural,
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

        let natural = [data.width, data.height];
        // Minified before the allocation rather than after it: the packer is
        // asked for the rectangle that is actually going to be uploaded, so an
        // over-ceiling source never takes (and then has to give back) space at
        // its declared extent.
        let (width, height) = fit_extent(natural, self.budget.atlas_size);
        let pixels = if [width, height] == natural {
            pixels
        } else {
            self.note_minify(key, natural, [width, height]);
            Arc::new(minify(&pixels, width, height))
        };

        let no_space = || ImageSkip::NoAtlasSpace {
            width,
            height,
            atlas_width: self.budget.atlas_size.0,
            atlas_height: self.budget.atlas_size.1,
        };

        let id = self
            .cache
            .allocate(width, height, ATLAS_PADDING)
            .map_err(|_| no_space())?;

        let Some(resource) = self.cache.get(id) else {
            // A successful allocation always populates its slot; refusing here
            // keeps the frame path free of the `expect` the reference renderer
            // takes at this same point.
            return Err(no_space());
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
                natural,
                may_have_transparency,
                last_seen: self.frame,
            },
        );
        self.uploads.push(ImageUpload {
            id,
            region,
            natural,
            pixels,
            may_have_transparency,
        });

        Ok(ResidentImage {
            id,
            region,
            natural,
            padding: u32::from(ATLAS_PADDING),
            may_have_transparency,
        })
    }

    /// Count and report one source downsampled to fit the atlas.
    ///
    /// Once per blob id, at warning level: a minified image is a silent quality
    /// decision the application may not have intended (a full-resolution photo
    /// where a thumbnail was wanted), so it says so — but a scene redrawing it
    /// every frame must not repeat the message.
    fn note_minify(&mut self, key: u64, natural: [u32; 2], fitted: [u32; 2]) {
        self.minified = self.minified.saturating_add(1);
        if !self.reported_minify.insert(key) {
            return;
        }
        log::warn!(
            "image {}x{} is larger than the {}x{} atlas budget; uploading a {}x{} box-filtered \
             copy instead (reported once per image)",
            natural[0],
            natural[1],
            self.budget.atlas_size.0,
            self.budget.atlas_size.1,
            fitted[0],
            fitted[1],
        );
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
    ///
    /// An upload still pending for that rectangle goes with the entry. It
    /// describes pixels no live entry claims any more, and writing them after
    /// the clear would put an evicted image back into a rectangle the packer has
    /// already handed to something else.
    fn release(&mut self, key: u64) {
        let Some(entry) = self.entries.remove(&key) else {
            return;
        };
        // The handle came from this cache's own `allocate`, so the atlas it
        // names exists and the deallocation cannot fail.
        self.cache.deallocate(entry.id);
        self.uploads.retain(|upload| upload.id != entry.id);

        // The plan survives an unacknowledged frame, so the same rectangle can
        // reach here twice before a single clear has been issued for it; one
        // clear is what the second one would write anyway.
        let region = padded(entry.region, ATLAS_PADDING);
        if !self.evictions.contains(&region) {
            self.evictions.push(region);
        }
    }
}

/// The extent `natural` is stored at inside an `atlas`-sized layer: itself when
/// it already fits, and otherwise the largest rectangle of the same aspect ratio
/// that does.
///
/// The scale is taken as the smaller of the two axis ratios and applied to both,
/// so the two axes of the paint transform stay in step — a per-axis fit would
/// stretch the image. Each axis is floored (never rounding *up* past the layer)
/// and then held at one texel: a source that scales below a whole texel still
/// has to have somewhere to be, and a zero-extent allocation is not a rectangle
/// the packer or the shader can address.
#[must_use]
pub fn fit_extent(natural: [u32; 2], atlas: (u32, u32)) -> (u32, u32) {
    let (width, height) = (natural[0], natural[1]);
    if width <= atlas.0 && height <= atlas.1 {
        return (width, height);
    }

    let scale = (f64::from(atlas.0) / f64::from(width.max(1)))
        .min(f64::from(atlas.1) / f64::from(height.max(1)));
    let axis = |value: u32, ceiling: u32| {
        let scaled = (f64::from(value) * scale).floor();
        // `as` on a non-finite or out-of-range float saturates in Rust, so the
        // clamps below are the whole check rather than a second line of defence.
        (scaled as u32).clamp(1, ceiling.max(1))
    };

    (axis(width, atlas.0), axis(height, atlas.1))
}

/// `source` box-filtered down to `width` x `height`.
///
/// Every output texel is the unweighted mean of the half-open source rectangle
/// it covers, computed on the premultiplied bytes the atlas stores — which is
/// the space the samples are composited in, so averaging there is what keeps a
/// partially transparent source from bleeding its colour outward.
///
/// Minification only: a request at or above the source's own extent copies the
/// texels straight across rather than inventing any, since a box filter has no
/// magnification arm and the caller never asks for one ([`fit_extent`] only ever
/// shrinks).
#[must_use]
pub fn minify(source: &Pixmap, width: u32, height: u32) -> Pixmap {
    let source_w = u32::from(source.width());
    let source_h = u32::from(source.height());
    let width = width.clamp(1, source_w.max(1));
    let height = height.clamp(1, source_h.max(1));

    let texels = source.data();
    let mut out = Vec::with_capacity((width as usize).saturating_mul(height as usize));
    let mut may_have_transparency = false;

    for y in 0..height {
        // Half-open source rows, derived from the output row rather than
        // accumulated, so rounding never leaves a gap or an overlap between
        // consecutive rows.
        let y0 = (u64::from(y) * u64::from(source_h) / u64::from(height)) as u32;
        let y1 = ((u64::from(y) + 1) * u64::from(source_h) / u64::from(height)) as u32;
        let y1 = y1.max(y0.saturating_add(1)).min(source_h);

        for x in 0..width {
            let x0 = (u64::from(x) * u64::from(source_w) / u64::from(width)) as u32;
            let x1 = ((u64::from(x) + 1) * u64::from(source_w) / u64::from(width)) as u32;
            let x1 = x1.max(x0.saturating_add(1)).min(source_w);

            let mut sum = [0_u64; 4];
            let mut count = 0_u64;
            for row in y0..y1 {
                let base = (row as usize).saturating_mul(source_w as usize);
                for column in x0..x1 {
                    let Some(texel) = texels.get(base.saturating_add(column as usize)) else {
                        continue;
                    };
                    sum[0] += u64::from(texel.r);
                    sum[1] += u64::from(texel.g);
                    sum[2] += u64::from(texel.b);
                    sum[3] += u64::from(texel.a);
                    count += 1;
                }
            }

            // A count of zero is unreachable — both ranges are non-empty by
            // construction — and flooring it at one rather than branching keeps
            // the divide-by-zero panic off the frame path (E17) while answering
            // a transparent texel if it ever were reachable, since the sums
            // would then be zero too.
            let divisor = count.max(1);
            // Round to nearest rather than truncating: a uniform source has to
            // come back bit-identical, which truncating an exact integer mean
            // already gives, but a near-uniform one must not drift a level
            // darker.
            let mean = |channel: usize| {
                ((sum[channel] + divisor / 2) / divisor).min(u64::from(u8::MAX)) as u8
            };

            let texel = PremulRgba8 {
                r: mean(0),
                g: mean(1),
                b: mean(2),
                a: mean(3),
            };
            may_have_transparency |= texel.a != u8::MAX;
            out.push(texel);
        }
    }

    // Both extents were clamped into `1..=u16::MAX` above (a source pixmap's own
    // extents are `u16`), so neither conversion can lose information.
    Pixmap::from_parts_with_opacity(
        out,
        width.min(u32::from(u16::MAX)) as u16,
        height.min(u32::from(u16::MAX)) as u16,
        may_have_transparency,
    )
}

/// `axis` held at or below `ceiling`, and never below the one texel an
/// addressable rectangle needs.
fn fit_axis(axis: u32, ceiling: u64) -> u32 {
    axis.min(u32::try_from(ceiling).unwrap_or(u32::MAX)).max(1)
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
            // Every frame here is a frame that reached the atlas.
            residency.acknowledge_plan();
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
        residency.acknowledge_plan();

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
    fn an_image_larger_than_the_atlas_is_minified_to_fit_rather_than_skipped() {
        let mut residency = residency();
        residency.begin_frame();

        // Twice the layer's width at a 16:1 aspect ratio: the width sets the
        // scale, and the height follows it rather than being fitted on its own.
        let resident = residency.resolve(&image(128, 8)).expect("minified to fit");

        assert_eq!(resident.region.size, [64, 4]);
        assert_eq!(resident.natural, [128, 8]);
        assert_eq!(resident.minify_scale(), Some((0.5, 0.5)));
        assert_eq!(residency.minified(), 1);
        assert_eq!(residency.skipped(), 0, "a big image is not a skipped one");
        assert_eq!(residency.uploads().len(), 1);
        let upload = &residency.uploads()[0];
        assert_eq!(
            (upload.pixels.width(), upload.pixels.height()),
            (64, 4),
            "the upload carries exactly the region's texels"
        );
    }

    #[test]
    fn an_atlas_with_no_room_left_is_still_a_skip() {
        // Minification fits an image to a LAYER, not to the space left in one:
        // a budget already full refuses the next allocation as it always did.
        let mut residency = ImageResidency::new(AtlasBudget {
            atlas_size: (64, 64),
            max_atlases: 1,
        });
        residency.begin_frame();
        residency
            .resolve(&image(64, 64))
            .expect("fills the one layer");

        let skip = residency
            .resolve(&image(32, 32))
            .expect_err("nothing is left to allocate from");
        assert!(matches!(skip, ImageSkip::NoAtlasSpace { .. }));
        assert_eq!(residency.skipped(), 1);
        assert_eq!(residency.minified(), 0);
    }

    #[test]
    fn a_minified_image_stays_resident_across_frames_at_its_declared_extent() {
        // Residency is keyed on the SOURCE extent, not the resident one: an
        // entry matched against its own minified rectangle would miss every
        // frame and re-upload the image on each of them.
        let mut residency = residency();
        let data = image(128, 8);

        for _ in 0..8 {
            residency.begin_frame();
            let resident = residency.resolve(&data).expect("minified to fit");
            assert_eq!(resident.natural, [128, 8]);
            residency.acknowledge_plan();
        }

        assert_eq!(residency.entry_count(), 1);
        assert_eq!(
            residency.minified(),
            1,
            "downsampled once, not once a frame"
        );
        assert!(
            residency.uploads().is_empty(),
            "no re-upload after the first"
        );
    }

    #[test]
    fn a_fit_extent_preserves_the_aspect_ratio_and_never_grows() {
        let atlas = (64, 64);
        // Already inside the layer: unchanged, both axes.
        assert_eq!(fit_extent([64, 64], atlas), (64, 64));
        assert_eq!(fit_extent([8, 4], atlas), (8, 4));

        // The `adv-huge-image` shape, at this budget's scale.
        assert_eq!(fit_extent([5000, 5000], (2048, 2048)), (2048, 2048));

        // The tighter axis sets the scale for both.
        assert_eq!(fit_extent([128, 8], atlas), (64, 4));
        assert_eq!(fit_extent([8, 128], atlas), (4, 64));
        assert_eq!(fit_extent([1000, 10], atlas), (64, 1));

        // A non-square layer is fitted per axis and still uniformly scaled.
        assert_eq!(fit_extent([400, 400], (100, 50)), (50, 50));
    }

    #[test]
    fn an_extreme_aspect_ratio_still_fits_at_one_texel_rather_than_none() {
        // 10000:1 into a 64-square layer scales the short axis to 0.0064 texels.
        // A zero extent is not a rectangle the packer or the shader can
        // address, so it is held at one.
        let fitted = fit_extent([10_000, 1], (64, 64));
        assert_eq!(fitted, (64, 1));

        let mut residency = residency();
        residency.begin_frame();
        let resident = residency
            .resolve(&image(10_000, 1))
            .expect("a sliver still becomes a rectangle");
        assert_eq!(resident.region.size, [64, 1]);
    }

    #[test]
    fn a_uniform_source_minifies_to_exactly_its_own_colour() {
        // What `adv-huge-image` rests on: a uniform source comes back bit
        // identical whatever the scale, so the case's `Exact` probe is a
        // property of the filter rather than a tolerance.
        let texel = PremulRgba8 {
            r: 255,
            g: 0,
            b: 255,
            a: 255,
        };
        let source = Pixmap::from_parts_with_opacity(vec![texel; 100 * 100], 100, 100, false);

        let small = minify(&source, 7, 3);
        assert_eq!((small.width(), small.height()), (7, 3));
        assert!(small.data().iter().all(|out| *out == texel));
        assert!(
            !small.may_have_transparency(),
            "an opaque source stays opaque"
        );
    }

    #[test]
    fn a_box_filter_averages_the_source_texels_each_output_covers() {
        // A 2x1 source of black and white halves down to one texel, which must
        // be their mean rather than either end.
        let black = PremulRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        let white = PremulRgba8 {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let source = Pixmap::from_parts_with_opacity(vec![black, white], 2, 1, false);

        let small = minify(&source, 1, 1);
        assert_eq!((small.width(), small.height()), (1, 1));
        // 255 / 2 rounded to nearest.
        assert_eq!(small.data()[0].r, 128);
        assert_eq!(small.data()[0].a, 255);

        // Transparency is recomputed from the result, not inherited: averaging
        // an opaque and a fully transparent texel produces a partial one.
        let clear = PremulRgba8 {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
        let mixed = Pixmap::from_parts_with_opacity(vec![white, clear], 2, 1, true);
        let small = minify(&mixed, 1, 1);
        assert_eq!(small.data()[0].a, 128);
        assert!(small.may_have_transparency());
    }

    #[test]
    fn minify_never_magnifies() {
        let texel = PremulRgba8 {
            r: 1,
            g: 2,
            b: 3,
            a: 255,
        };
        let source = Pixmap::from_parts_with_opacity(vec![texel; 4], 2, 2, false);

        // A request above the source's own extent is clamped to it rather than
        // inventing texels the box filter has no arm for.
        let same = minify(&source, 8, 8);
        assert_eq!((same.width(), same.height()), (2, 2));
        assert!(same.data().iter().all(|out| *out == texel));
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
    fn reading_the_plan_leaves_it_pending_and_acknowledging_it_clears_it() {
        let mut residency = residency();
        residency.begin_frame();
        residency.resolve(&image(8, 8)).expect("fits");

        let (evictions, uploads) = residency.plan();
        assert!(evictions.is_empty());
        assert_eq!(uploads.len(), 1);
        assert!(
            residency.has_pending_plan(),
            "reading the plan is not servicing it"
        );

        residency.acknowledge_plan();
        assert!(!residency.has_pending_plan());
        assert!(residency.uploads().is_empty());
        assert!(residency.evictions().is_empty());
    }

    #[test]
    fn an_unacknowledged_upload_is_re_emitted_until_it_is_serviced() {
        // The invariant the whole pending plan exists for: a frame refused
        // after compiling never wrote these texels, so the entry map claiming
        // the image is resident has to stay answerable by a later frame's plan.
        let mut residency = residency();
        let data = image(8, 8);

        residency.begin_frame();
        let first = residency.resolve(&data).expect("fits");
        let region = residency.uploads()[0].region;

        for _ in 0..4 {
            residency.begin_frame();
            let again = residency.resolve(&data).expect("still resident");
            assert_eq!(again.region, first.region, "residency does not move");
            assert_eq!(
                residency.uploads().len(),
                1,
                "the same upload is re-offered, never duplicated"
            );
            assert_eq!(residency.uploads()[0].region, region);
        }

        residency.acknowledge_plan();
        residency.begin_frame();
        residency.resolve(&data).expect("still resident");
        assert!(
            residency.uploads().is_empty(),
            "a serviced upload is never offered again"
        );
    }

    #[test]
    fn reaping_an_unacknowledged_entry_withdraws_its_upload_with_it() {
        // Otherwise the plan would clear the rectangle and then write the
        // evicted image straight back into it, over whatever the packer handed
        // that space to next.
        let mut residency = residency();
        let data = image(8, 8);

        residency.begin_frame();
        let resident = residency.resolve(&data).expect("fits");
        assert_eq!(residency.uploads().len(), 1);

        for _ in 0..=MAX_UNSEEN_FRAMES {
            residency.begin_frame();
        }

        assert_eq!(residency.entry_count(), 0);
        assert!(
            residency.uploads().is_empty(),
            "the reaped entry's unserviced upload goes with it"
        );
        assert_eq!(
            residency.evictions(),
            &[padded(resident.region, ATLAS_PADDING)]
        );
    }

    #[test]
    fn every_pending_upload_lies_inside_the_atlas_its_layer_count_asks_for() {
        // What keeps a consumer from recording where an image lives and then
        // sampling a rectangle the array refused to write: the plan can only
        // ever name regions the budget and the reported depth already admit.
        let mut residency = residency();
        residency.begin_frame();
        residency.resolve(&image(64, 48)).expect("fills a layer");
        residency.resolve(&image(64, 48)).expect("takes the next");

        let budget = residency.budget();
        let layers = residency.layers();
        assert_eq!(layers, 2);
        for upload in residency.uploads() {
            assert!(
                budget.contains(upload.region, layers),
                "{:?} is outside a {layers}-layer {budget:?}",
                upload.region
            );
        }
    }

    #[test]
    fn an_override_redistributes_a_tiers_memory_but_never_exceeds_it() {
        let tier = AtlasBudget::DESKTOP;

        // The pathological override the adapter alone would admit: a
        // 16384-square layer is a gibibyte on its own, so clamping the layer
        // count could not have brought it inside the tier's 128 MiB.
        let huge = AtlasBudget {
            atlas_size: (16_384, 16_384),
            max_atlases: tier.max_atlases,
        }
        .within_total_bytes(tier.total_bytes());
        assert!(huge.total_bytes() <= tier.total_bytes());
        assert_eq!(huge.atlas_size.0, huge.atlas_size.1, "aspect preserved");
        assert!(huge.max_atlases >= 1);

        // A modest override is left exactly as asked for.
        let modest = AtlasBudget {
            atlas_size: (1024, 1024),
            max_atlases: tier.max_atlases,
        }
        .within_total_bytes(tier.total_bytes());
        assert_eq!(modest.atlas_size, (1024, 1024));
        assert_eq!(modest.max_atlases, tier.max_atlases);

        // An extreme aspect ratio floors one axis to nothing; it is raised back
        // to a texel and the other axis gives the room up, so the pair still
        // fits and neither axis is zero.
        let sliver = AtlasBudget {
            atlas_size: (65_535, 4),
            max_atlases: 1,
        }
        .within_total_bytes(64);
        assert!(sliver.atlas_size.0 >= 1 && sliver.atlas_size.1 >= 1);
        assert!(sliver.total_bytes() <= 64);
    }
}
