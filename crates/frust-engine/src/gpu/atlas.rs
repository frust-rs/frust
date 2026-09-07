//! The image atlas array, and the encoded-image record the strip shader reads
//! it through.
//!
//! Two halves, split on the same line the rest of [`crate::gpu`] is split on.
//!
//! - **Pure decisions over plain values.** The texture descriptor, the packing
//!   of an [`AtlasRegion`] and a sampler into a
//!   [`GpuEncodedImage`](super::GpuEncodedImage), and the natural-to-device
//!   transform an image paint carries — all host-testable with no device.
//! - **One live-GPU type.** [`AtlasArray`] owns the `Rgba8Unorm` `D2Array`
//!   texture the shader's `atlas_texture_array` binding samples, grows it a
//!   layer at a time, writes newly resident regions and clears evicted ones.
//!
//! ## Why the atlas is not pooled
//!
//! [`crate::gpu::targets`] pools every *transient* the engine allocates, on the
//! rule that a target nothing outlives the frame should be reused rather than
//! reallocated. The atlas is the opposite kind of resource: its whole purpose
//! is that an image uploaded on one frame is still there on the next thousand,
//! so it is owned outright for the life of the renderer and reclaimed a region
//! at a time by [`crate::cache::images`]'s age-based reap. Handing it to the
//! pool would make residency a lie.
//!
//! ## Growth and clearing
//!
//! A `wgpu` texture's array-layer count is fixed at creation, so growing the
//! array means creating a deeper texture and copying every existing layer
//! across — which is why the array carries `COPY_SRC` alongside `COPY_DST`.
//! Growth is rare (a layer holds a whole mobile budget's worth of images) and
//! never shrinks: an atlas that grew to four layers under load keeps them.
//!
//! ### Why growth submits a command buffer of its own
//!
//! That copy is the one piece of engine work that cannot ride the frame's
//! encoder. `wgpu` flushes the queued writes pending at a submit *before* the
//! command buffers of that same submit, so a copy recorded into the frame's
//! encoder would execute after the frame's own `write_texture` uploads and
//! clears — restoring the pre-growth contents of every layer they had just
//! written, permanently (residency schedules an upload on a miss, and the image
//! is not a miss any more). Ordering is therefore established by submitting the
//! copy on its own, ahead of the frame's writes being issued at all.
//!
//! That is a **maintenance** submit, not scene work: it carries one texture
//! copy, no pass and no draw, and it happens only on the rare frame that grows
//! the array. [`crate::renderer::EngineRenderer::encode`]'s contract that the
//! engine never submits *the caller's* encoder, and records no scene work
//! anywhere else, is untouched — the caller's encoder is neither read nor
//! finished here.
//!
//! Clearing an evicted region writes transparent texels through the queue
//! rather than drawing a scissored pass. Both reach the same result; the queue
//! write needs no pipeline, no render pass and no bind group, and eviction is
//! a once-in-sixty-frames event whose cost is a zeroed staging buffer the size
//! of the region.
//!
//! ## Rendering *into* the atlas
//!
//! Everything above writes the atlas from the host. A glyph is different: it
//! has no pixels until something rasterizes its outline, and `glifo`'s
//! [`AtlasCacher::Enabled`](glifo::AtlasCacher::Enabled) path does not
//! rasterize one — it records the fills into a per-page
//! [`AtlasCommandRecorder`] and leaves the pixels to whoever owns the atlas.
//! [`AtlasRenderer`] is that owner on this tier: it replays each dirty page's
//! commands into a strip pass whose colour attachment is that page's own array
//! layer.
//!
//! Three orderings make the difference between a correct glyph and a stale
//! one, and all three are this type's to keep.
//!
//! 1. **Clears before uploads.** A rectangle an eviction freed can be handed
//!    straight back out to a different glyph on the same frame, so zeroing it
//!    after that glyph's pixels landed would erase the glyph that just moved
//!    in. Both are queue writes, which execute in the order they are issued —
//!    so the order they are issued in is the whole guarantee.
//! 2. **Both before the pass.** `wgpu` flushes the writes queued at a submit
//!    before that submit's command buffers, so a page's replay pass sees the
//!    clears and the bitmap uploads already applied to the layer it composites
//!    onto, without anything having to be said about it.
//! 3. **The pass before the scene's.** A glyph the scene pass samples out of
//!    the atlas has to be *in* the atlas by then, and the scene pass lives in
//!    an encoder this crate does not own and never submits. So the replay pass
//!    goes into an encoder of [`AtlasRenderer`]'s own and is submitted before
//!    it returns — the sanctioned exception to `frust_gpu::CommandBuffer`'s
//!    single-submit borrowing contract, named there as the glyph-atlas upload
//!    carve-out and the same one [`AtlasArray::ensure_layers`] already takes
//!    for the growth copy.
//!
//! One submit per dirty page rather than one for all of them: the coverage a
//! page's strips index is uploaded to a shared alpha texture, and a second
//! queue write to that texture in the same submit would overwrite the first
//! before either pass ran. Pages are dirty only on a frame that missed a glyph,
//! and there is one page in the overwhelming case, so the extra submit is a
//! per-miss cost rather than a per-frame one.
//!
//! ### What the replay does not lower
//!
//! Turning a recorded command stream into strips is compiler work, and
//! [`crate::compile`] already depends on this module — so the lowering is a
//! closure the caller supplies ([`AtlasRenderer::render_pending`]) rather than
//! a dependency taken the other way. What this module contributes to it is
//! [`push_solid_strips`], the pure expansion of one solid-painted strip run
//! into instances, which is the whole of an outline glyph's lowering.

use vello_common::encode::EncodedImage;
use vello_common::kurbo::{Affine, Rect, Vec2};
use vello_common::paint::{ImageSource, Tint, TintMode};
use vello_common::strip::Strip;

use frust_gpu::TierCaps;
use glifo::atlas::PendingBitmapUpload;
use glifo::{AtlasCommandRecorder, GlyphAtlas, PendingClearRect};

use crate::EngineError;
use crate::cache::images::{ATLAS_FORMAT_BYTES, AtlasRegion, ResidentImage};
use crate::diag::{EngineSpan, FrameTimestamps};

use super::GpuEncodedPaint;
use super::config::GpuConfig;
use super::paint_texture::GpuEncodedImage;
use super::strips::{GpuStrip, PaintType, StripDraw, pack_paint_descriptor};

/// The texture format the atlas array stores premultiplied image texels in.
pub const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The usages the atlas array is created with.
///
/// `TEXTURE_BINDING` for the strip shader's own sampling, `COPY_DST` for a
/// region upload or clear, `COPY_SRC` for the layer copy that growth performs,
/// and `RENDER_ATTACHMENT` so a scissored clear pass remains available to a
/// later caller that wants one — the same four the reference renderer creates
/// its atlas with.
pub const ATLAS_USAGES: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::RENDER_ATTACHMENT);

/// The most atlas layers the encoded-image record's `atlas_index` field can
/// name (eight bits).
pub const MAX_ATLAS_INDEX: u32 = 0xFF;

/// The descriptor for an atlas array of `width` x `height` texels over
/// `layers` array layers.
///
/// `layers` is raised to at least **two**, not one. A texture with zero array
/// layers cannot be created at all, which would be reason enough for a floor
/// of one — but wgpu-hal 30.0.1's GLES backend picks a texture's GL target
/// from the descriptor alone and never consults the view dimension a caller
/// binds it through (`get_info_from_desc`, wgpu-hal `src/gles/mod.rs:513-530`:
/// `(false, 1) => TEXTURE_2D`). A one-layer array descriptor therefore binds
/// as plain `GL_TEXTURE_2D` while [`super::atlas`]'s strip shader always
/// samples this texture as a `sampler2DArray`
/// (`crates/frust-engine/shaders/strip.wgsl`'s `texture_2d_array<f32>`
/// binding) — target and sampler disagree, the texture reads as incomplete
/// per GLES 3.0 §3.8.2, and every sample returns `(0, 0, 0, 1)`: a solid box
/// instead of a glyph, a solid black rect instead of an image. Two layers is
/// the smallest depth the heuristic reads as `TEXTURE_2D_ARRAY`, so it is the
/// floor a renderer with a single resident image or glyph page still has to
/// allocate at. See wgpu upstream issues #1614 and #1574; this is a
/// workaround, not the fix, and is meant to come out once wgpu-hal honours
/// the view dimension (tracked in `docs/LIMITATIONS.md`).
#[must_use]
pub fn atlas_texture_descriptor(
    width: u32,
    height: u32,
    layers: u32,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("frust-engine image atlas array"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: layers.max(2),
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: ATLAS_FORMAT,
        usage: ATLAS_USAGES,
        view_formats: &[],
    }
}

/// The `D2Array` view descriptor the strip shader's atlas binding expects.
#[must_use]
pub fn atlas_view_descriptor() -> wgpu::TextureViewDescriptor<'static> {
    wgpu::TextureViewDescriptor {
        label: Some("frust-engine image atlas array view"),
        format: None,
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        aspect: wgpu::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: None,
        base_array_layer: 0,
        array_layer_count: None,
        usage: None,
    }
}

/// The single-layer `D2` view descriptor a render pass attaches one atlas
/// layer through.
///
/// Deliberately not [`atlas_view_descriptor`]'s shape: a colour attachment has
/// to name exactly one layer, so this is a plain 2D view over
/// `layer`, not the `D2Array` view the shader samples the whole array through.
/// The two coexist on the same texture — one bound for sampling by the scene
/// pass, one attached for writing by the replay pass — which is why
/// [`ATLAS_USAGES`] carries both `TEXTURE_BINDING` and `RENDER_ATTACHMENT`.
#[must_use]
pub fn atlas_layer_view_descriptor(layer: u32) -> wgpu::TextureViewDescriptor<'static> {
    wgpu::TextureViewDescriptor {
        label: Some("frust-engine image atlas layer target"),
        format: None,
        dimension: Some(wgpu::TextureViewDimension::D2),
        aspect: wgpu::TextureAspect::All,
        base_mip_level: 0,
        mip_level_count: None,
        base_array_layer: layer,
        array_layer_count: Some(1),
        usage: None,
    }
}

/// The atlas array texture, its view, and the layer count both were created
/// at.
///
/// Created lazily by the first frame that makes an image resident, then grown
/// only. The view is kept beside the texture because a bind group is built
/// against it and has to be rebuilt whenever growth replaces the texture —
/// [`generation`](Self::generation) is what tells a caller that happened.
#[derive(Debug)]
pub struct AtlasArray {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    layers: u32,
    generation: u64,
}

impl AtlasArray {
    /// An atlas array of `width` x `height` texels with a single layer.
    #[must_use]
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::with_layers(device, width, height, 1)
    }

    /// An atlas array of `width` x `height` texels over `layers` layers.
    #[must_use]
    pub fn with_layers(device: &wgpu::Device, width: u32, height: u32, layers: u32) -> Self {
        let descriptor = atlas_texture_descriptor(width, height, layers);
        let texture = device.create_texture(&descriptor);
        let view = texture.create_view(&atlas_view_descriptor());

        Self {
            texture,
            view,
            width: descriptor.size.width,
            height: descriptor.size.height,
            layers: descriptor.size.depth_or_array_layers,
            generation: 0,
        }
    }

    /// The array texture the shader samples.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The `D2Array` view a bind group binds.
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Extent of each layer, in texels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How many array layers currently exist.
    #[must_use]
    pub fn layers(&self) -> u32 {
        self.layers
    }

    /// A render-attachment view over one array layer, or `None` when the array
    /// has no such layer.
    ///
    /// Minted per use rather than cached alongside [`view`](Self::view): a
    /// layer target is wanted only on a frame that has glyph pixels to
    /// rasterize, while the sampling view is bound by every frame, and holding
    /// one view per layer for the array's lifetime would keep a handle alive
    /// per layer for a path most frames never take.
    #[must_use]
    pub fn layer_view(&self, layer: u32) -> Option<wgpu::TextureView> {
        (layer < self.layers).then(|| {
            self.texture
                .create_view(&atlas_layer_view_descriptor(layer))
        })
    }

    /// How many times growth has replaced the underlying texture.
    ///
    /// A bind group built against [`view`](Self::view) stays valid for as long
    /// as this value does not change.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Grow the array to hold at least `layers` layers, preserving every
    /// existing layer's texels.
    ///
    /// Returns whether the texture was replaced — the signal a caller needs to
    /// rebuild its bind group. A request at or below the current depth, or one
    /// past [`MAX_ATLAS_INDEX`], is a no-op: the encoded-image record cannot
    /// name a layer the shader could not address, so refusing here is what
    /// keeps an unaddressable layer from being created at all.
    ///
    /// The old-to-new copy is recorded into a command encoder of this method's
    /// own and submitted before returning, rather than into the frame's. That is
    /// an ordering requirement rather than a convenience — see the module doc's
    /// *Why growth submits a command buffer of its own*. Call it before the
    /// frame's atlas writes are issued; anything already queued is flushed by
    /// this submit and so lands in the *old* texture.
    pub fn ensure_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: u32,
    ) -> bool {
        if layers <= self.layers || layers > MAX_ATLAS_INDEX + 1 {
            return false;
        }

        let grown = Self::with_layers(device, self.width, self.height, layers);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine image atlas growth"),
        });
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            grown.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: self.layers,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let generation = self.generation.saturating_add(1);
        *self = grown;
        self.generation = generation;
        true
    }

    /// Write `pixels` into `region`.
    ///
    /// `pixels` must be `region`'s own extent in premultiplied `Rgba8Unorm`,
    /// row-major and unpadded — exactly what
    /// [`crate::cache::images::ImageUpload`] carries. A slice that does not
    /// match, or a region outside the array, is refused rather than handed to
    /// the queue, which would validate it into a device error mid-frame.
    pub fn write_region(&self, queue: &wgpu::Queue, region: AtlasRegion, pixels: &[u8]) -> bool {
        if !self.contains(region) || pixels.len() != region.byte_len() {
            return false;
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.offset[0],
                    y: region.offset[1],
                    z: region.layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(region.bytes_per_row()),
                rows_per_image: Some(region.size[1]),
            },
            wgpu::Extent3d {
                width: region.size[0],
                height: region.size[1],
                depth_or_array_layers: 1,
            },
        );
        true
    }

    /// Clear `region` to transparent texels.
    ///
    /// This is what makes an eviction observable as *absence* rather than as a
    /// stale image: the rectangle a reaped entry gave back is zeroed before a
    /// later allocation can hand part of it to something smaller, so a sample
    /// that strays into the unwritten remainder reads transparent black rather
    /// than the previous tenant's pixels.
    pub fn clear_region(&self, queue: &wgpu::Queue, region: AtlasRegion) -> bool {
        if !self.contains(region) {
            return false;
        }
        let zeros = vec![0_u8; region.byte_len()];
        self.write_region(queue, region, &zeros)
    }

    /// Whether `region` lies wholly inside this array.
    #[must_use]
    pub fn contains(&self, region: AtlasRegion) -> bool {
        !region.is_empty()
            && region.layer < self.layers
            && region.offset[0].saturating_add(region.size[0]) <= self.width
            && region.offset[1].saturating_add(region.size[1]) <= self.height
    }
}

/// The bytes a region's texels occupy — the length
/// [`AtlasArray::write_region`] requires of its slice.
#[must_use]
pub fn region_byte_len(region: AtlasRegion) -> usize {
    (region.size[0] as usize)
        .saturating_mul(region.size[1] as usize)
        .saturating_mul(ATLAS_FORMAT_BYTES as usize)
}

/// The affine mapping an image's natural pixel rectangle
/// `(0, 0, width, height)` onto `dest`, composed under `transform`.
///
/// The same composition `frust-render`'s CPU-tier lowering applies, so the two
/// tiers place an image identically: a natural-size draw under the widget's own
/// transform, translated to `dest`'s origin and scaled to `dest`'s extent.
/// `None` for a degenerate natural size, which has no scale to derive.
#[must_use]
pub fn natural_to_dest(transform: Affine, natural: (u32, u32), dest: Rect) -> Option<Affine> {
    let natural_w = f64::from(natural.0);
    let natural_h = f64::from(natural.1);
    if natural_w <= 0.0 || natural_h <= 0.0 {
        return None;
    }

    Some(
        transform
            * Affine::translate((dest.x0, dest.y0))
            * Affine::scale_non_uniform(dest.width() / natural_w, dest.height() / natural_h),
    )
}

/// The per-pixel advances in image space an encoded image carries, derived
/// from its already-inverted transform.
///
/// The linear part only: an advance is a direction, so the translation is
/// dropped. `vello_common` computes the same pair internally and keeps it
/// private, so it is restated here rather than reached for.
#[must_use]
pub fn x_y_advances(transform: Affine) -> (Vec2, Vec2) {
    let c = transform.as_coeffs();
    (Vec2::new(c[0], c[1]), Vec2::new(c[2], c[3]))
}

/// Packs an image's width and height into one word, width in the high half.
#[must_use]
pub const fn pack_image_size(width: u16, height: u16) -> u32 {
    ((width as u32) << 16) | (height as u32)
}

/// Packs an image's atlas offset into one word, x in the high half.
#[must_use]
pub const fn pack_image_offset(x: u16, y: u16) -> u32 {
    ((x as u32) << 16) | (y as u32)
}

/// Packs sampling quality (bits 0-1), the two extend modes (bits 2-3 and 4-5),
/// the atlas layer (bits 6-13) and the source kind (bit 14) into one word.
///
/// Each field is masked to its width rather than reported: every input is
/// produced by this crate's own encoding, and the preconditions are checked in
/// debug builds.
#[must_use]
pub fn pack_image_params(
    quality: u32,
    extend_x: u32,
    extend_y: u32,
    atlas_index: u32,
    is_external: bool,
) -> u32 {
    debug_assert!(quality <= 3, "quality must fit two bits");
    debug_assert!(extend_x <= 3, "extend_x must fit two bits");
    debug_assert!(extend_y <= 3, "extend_y must fit two bits");
    debug_assert!(
        atlas_index <= MAX_ATLAS_INDEX,
        "atlas index {atlas_index} exceeds {MAX_ATLAS_INDEX}",
    );

    (u32::from(is_external) << 14)
        | ((atlas_index & MAX_ATLAS_INDEX) << 6)
        | ((extend_y & 0b11) << 4)
        | ((extend_x & 0b11) << 2)
        | (quality & 0b11)
}

/// The premultiplied colour and mode an optional tint packs to.
///
/// With no tint the colour is all-ones under [`TintMode::Multiply`], which
/// leaves the sampled texel exactly as it was — the shader always applies a
/// tint, so "no tint" has to be expressed as an identity one rather than as a
/// branch.
#[must_use]
pub fn pack_tint(tint: Option<Tint>) -> (u32, u32) {
    match tint {
        Some(tint) => (
            tint.color.premultiply().to_rgba8().to_u32(),
            tint.mode.as_u32(),
        ),
        None => (u32::MAX, TintMode::Multiply.as_u32()),
    }
}

/// The shader's extend-mode numbering.
pub const fn extend_mode(extend: peniko::Extend) -> u32 {
    match extend {
        peniko::Extend::Pad => 0,
        peniko::Extend::Repeat => 1,
        peniko::Extend::Reflect => 2,
    }
}

/// Lower one encoded image paint into the record the strip shader samples.
///
/// `resident` is where the image's texels were made resident, which only a
/// caller that already serviced the frame's residency can supply — the same
/// shape [`super::paint_texture::lower_encoded_paint`] uses for a gradient's
/// baked ramp. An image whose source is not the handle `resident` names
/// answers `None`, so a paint encoded against a different residency becomes a
/// dropped draw rather than a wrongly-addressed one.
///
/// The encoded transform is already the inverse mapping — device space into
/// image space — because that is the direction the shader applies it in;
/// narrowing it to `f32` here is what the record and the WGSL both read it at.
#[must_use]
pub fn lower_encoded_image(
    image: &EncodedImage,
    resident: &ResidentImage,
) -> Option<GpuEncodedPaint> {
    match &image.source {
        ImageSource::OpaqueId { id, .. } if *id == resident.id => {}
        _ => return None,
    }

    let region = resident.region;
    let (tint, tint_mode) = pack_tint(image.tint);

    Some(GpuEncodedPaint::Image(GpuEncodedImage {
        image_params: pack_image_params(
            image.sampler.quality as u32,
            extend_mode(image.sampler.x_extend),
            extend_mode(image.sampler.y_extend),
            region.layer,
            false,
        ),
        image_size: pack_image_size(truncate_u16(region.size[0]), truncate_u16(region.size[1])),
        image_offset: pack_image_offset(
            truncate_u16(region.offset[0]),
            truncate_u16(region.offset[1]),
        ),
        transform: image.transform.as_coeffs().map(|coeff| coeff as f32),
        tint,
        tint_mode,
        image_padding: resident.padding,
    }))
}

/// `value` narrowed to the `u16` the record's packed halves hold.
///
/// Saturating rather than wrapping: every caller has already passed the
/// residency's own `u16` ceiling, so this can only ever be the identity, and a
/// saturation is the harmless reading if that ever stops being true.
const fn truncate_u16(value: u32) -> u16 {
    if value > u16::MAX as u32 {
        u16::MAX
    } else {
        value as u16
    }
}

/// The width of the stand-in encoded-paint texture an atlas pass binds.
///
/// One texel, because nothing an atlas pass draws reads that texture: a
/// replayed glyph outline paints solid, and a solid instance carries its colour
/// in its own payload. A binding still has to be filled for the pass to
/// validate, and the config the shader reconstructs the texture's width from
/// has to agree with it — a power of two, which one is.
const PLACEHOLDER_PAINT_TEX_WIDTH: u32 = 1;

/// Instances an atlas pass's buffer holds before it is grown for a page that
/// needs more.
///
/// A page of glyph outlines is a few hundred strips in the ordinary case; this
/// floor keeps the first miss from allocating a buffer measured in single
/// instances and then reallocating it four times on the way up.
const MIN_ATLAS_INSTANCES: u64 = 256;

/// Expands one solid-painted strip run into the instances that draw it,
/// appending to `out`.
///
/// This is the whole of an outline glyph's lowering: `glifo` records a glyph as
/// a fill of a path in one colour, so every instance of the run repeats the
/// same premultiplied `payload` and the same solid paint descriptor, and a
/// position-sampled paint's per-instance re-evaluation never arises.
///
/// `strips` is a *generation's* strips, sentinel included: a strip's extent is
/// carried by the strip after it, so instances come from consecutive pairs and
/// the trailing sentinel becomes none. Passing a run without its sentinel
/// silently drops the last span.
///
/// Returns how many instances were appended, which is neither `strips.len()`
/// nor bounded by it — a pair can contribute a span, a winding gap fill, both,
/// or neither.
pub fn push_solid_strips(
    strips: &[Strip],
    payload: u32,
    depth: u32,
    out: &mut Vec<GpuStrip>,
) -> u32 {
    let draw = StripDraw {
        payload,
        paint: pack_paint_descriptor(PaintType::Solid, 0),
        depth_index: depth,
    };

    let before = out.len();
    for pair in strips.windows(2) {
        let span = GpuStrip::from_strip_pair(&pair[0], &pair[1], draw);
        if span.width > 0 {
            out.push(span);
        }
        if let Some(gap) = GpuStrip::gap_fill(&pair[0], &pair[1], draw) {
            out.push(gap);
        }
    }
    u32::try_from(out.len().saturating_sub(before)).unwrap_or(u32::MAX)
}

/// One page's lowered replay: the instances to draw and the coverage they
/// index.
///
/// Reused across pages and across frames rather than allocated per page — the
/// two vectors are the only heap a replay costs once they have grown, which is
/// the point of handing them to the lowering closure instead of taking a fresh
/// pair back from it.
///
/// The two are kept together because a strip instance's alpha column is only
/// meaningful against the coverage buffer generated alongside it, the same
/// pairing [`crate::compile::CompiledFrame`] keeps for a frame.
#[derive(Debug, Default)]
pub struct AtlasPageBuffers {
    /// The page's strip instances, in the order they are drawn.
    pub instances: Vec<GpuStrip>,
    /// The coverage bytes [`instances`](Self::instances) index.
    pub alphas: Vec<u8>,
}

impl AtlasPageBuffers {
    /// Empties both buffers, keeping their capacity for the next page.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.alphas.clear();
    }

    /// Whether this page would draw nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

/// What one call to [`AtlasRenderer::render_pending`] serviced.
///
/// Every field is a count rather than a flag because each names work that
/// either reached the atlas or did not, and a glyph that silently failed to
/// rasterize is otherwise indistinguishable from one the text never asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtlasRenderReport {
    /// Evicted rectangles zeroed, ahead of every upload.
    pub cleared: u32,
    /// Bitmap (and COLR pixmap) glyphs written through the queue.
    pub uploaded: u32,
    /// Pages whose recorded commands were replayed into a pass.
    pub pages: u32,
    /// Strip instances those passes drew.
    pub instances: u32,
    /// Pages, clears and uploads that were refused — a page the lowering
    /// declined, a layer the array does not have, coverage past the alpha
    /// texture's ceiling, or a region the array would not take.
    pub refused: u32,
}

impl AtlasRenderReport {
    /// Whether this call did anything at all.
    ///
    /// The steady state: text that hit the cache on every glyph queues no
    /// upload, frees no rectangle and dirties no page, so an atlas renderer
    /// driven every frame submits nothing on almost all of them.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One resource texture the atlas pass owns, and the extent it holds.
#[derive(Debug)]
struct AtlasResourceTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl AtlasResourceTexture {
    fn new(device: &wgpu::Device, descriptor: &wgpu::TextureDescriptor<'_>) -> Self {
        let texture = device.create_texture(descriptor);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width: descriptor.size.width,
            height: descriptor.size.height,
        }
    }

    fn copy_target(&self) -> wgpu::TexelCopyTextureInfo<'_> {
        wgpu::TexelCopyTextureInfo {
            texture: &self.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        }
    }

    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        }
    }
}

/// The 1x1 stand-ins for the strip shader's bindings an atlas pass has nothing
/// real for.
///
/// Every declared binding has to be filled for a pass to validate, whether or
/// not an instance samples it. Three of these five are stand-ins for the same
/// reason the frame path's are — no layer input, no external texture, no
/// gradient ramp. The other two are stand-ins for a reason particular to this
/// pass: the *atlas array* itself is deliberately not bound here, because the
/// pass is writing one of its layers, and a texture attached for writing cannot
/// also be bound for sampling in the same pass; and the encoded-paint texture
/// is unused because a replayed outline paints solid.
#[derive(Debug)]
struct AtlasPlaceholders {
    layer_input: wgpu::TextureView,
    array: wgpu::TextureView,
    external: wgpu::TextureView,
    paints: wgpu::TextureView,
    gradients: wgpu::TextureView,
}

impl AtlasPlaceholders {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            layer_input: placeholder(
                device,
                "frust-engine atlas pass layer input",
                ATLAS_FORMAT,
                false,
            ),
            array: placeholder(device, "frust-engine atlas pass array", ATLAS_FORMAT, true),
            external: placeholder(
                device,
                "frust-engine atlas pass external",
                ATLAS_FORMAT,
                false,
            ),
            paints: placeholder(
                device,
                "frust-engine atlas pass paints",
                super::RESOURCE_TEXTURE_FORMAT,
                false,
            ),
            gradients: placeholder(
                device,
                "frust-engine atlas pass gradients",
                ATLAS_FORMAT,
                false,
            ),
        }
    }
}

/// A 1x1 sampled-only texture's view, as a plain 2D texture or a 2D array.
///
/// The array variant allocates **two** layers, not one, for the same reason
/// [`atlas_texture_descriptor`] does: wgpu-hal 30.0.1's GLES backend derives
/// the GL target from the descriptor's layer count alone, and a one-layer
/// array descriptor binds as `GL_TEXTURE_2D` rather than
/// `GL_TEXTURE_2D_ARRAY` (see that function's doc comment for the file:line
/// and upstream issue refs). Only layer zero of the two is ever sampled here.
fn placeholder(
    device: &wgpu::Device,
    label: &'static str,
    format: wgpu::TextureFormat,
    array: bool,
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: if array { 2 } else { 1 },
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some(label),
        dimension: Some(if array {
            wgpu::TextureViewDimension::D2Array
        } else {
            wgpu::TextureViewDimension::D2
        }),
        ..Default::default()
    })
}

/// The GPU home for glyph pixels: replays `glifo`'s recorded atlas commands
/// into the atlas array's own layers, and services the two pixel drains that
/// have to be ordered around them.
///
/// Owned for the life of a renderer, beside the [`AtlasArray`] it writes rather
/// than inside it — the array is a texture a scene pass samples, this is the
/// machinery a *replay* pass needs, and most renderers never take the replay
/// path at all. See the module doc's *Rendering into the atlas* for the three
/// orderings this type exists to keep.
///
/// Nothing here is reached by the frame path until the text backend turns
/// `glifo`'s atlas cacher on; a renderer that holds one and drives it every
/// frame submits nothing on any frame that missed no glyph
/// ([`AtlasRenderReport::is_empty`]).
#[derive(Debug)]
pub struct AtlasRenderer {
    /// Coverage for the page currently being drawn. Shared across pages and
    /// rewritten per page, which is why each page is its own submit.
    alphas: AtlasResourceTexture,
    /// The viewport uniform, rewritten per page — identical for every layer of
    /// one array, since every layer has the array's own extent.
    config: wgpu::Buffer,
    /// The instance buffer every page's strips are written to the head of.
    instances: wgpu::Buffer,
    instance_capacity: u64,
    placeholders: AtlasPlaceholders,
    /// Handed to the lowering closure page by page, so a replay allocates
    /// nothing once these have grown.
    buffers: AtlasPageBuffers,
}

impl AtlasRenderer {
    /// An atlas renderer for `caps`' adapter.
    ///
    /// The coverage texture is created at its minimum height and grown by the
    /// first page that needs more, the same growth-only rule every other
    /// resource texture in this crate follows.
    #[must_use]
    pub fn new(device: &wgpu::Device, caps: &TierCaps) -> Self {
        let dim = caps.resource_texture_dim;
        Self {
            alphas: AtlasResourceTexture::new(
                device,
                &super::alpha_texture_descriptor(dim, super::MIN_RESOURCE_TEXTURE_HEIGHT),
            ),
            config: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frust-engine atlas pass config uniform"),
                size: GpuConfig::SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            instances: device.create_buffer(&instance_descriptor(
                MIN_ATLAS_INSTANCES * size_of::<GpuStrip>() as u64,
            )),
            instance_capacity: MIN_ATLAS_INSTANCES * size_of::<GpuStrip>() as u64,
            placeholders: AtlasPlaceholders::new(device),
            buffers: AtlasPageBuffers::default(),
        }
    }

    /// The coverage texture's current extent in texels.
    #[must_use]
    pub fn alpha_texture_size(&self) -> (u32, u32) {
        (self.alphas.width, self.alphas.height)
    }

    /// The instance buffer's current capacity in bytes.
    #[must_use]
    pub fn instance_capacity(&self) -> u64 {
        self.instance_capacity
    }

    /// Zero one evicted rectangle, answering whether the array took it.
    ///
    /// A queue write, not a scissored pass, for [`AtlasArray::clear_region`]'s
    /// reason — and issued through the queue rather than an encoder so that it
    /// is guaranteed to land before any pass of the same submit, which is what
    /// lets a rectangle freed this frame be drawn into this frame.
    pub fn clear_rect(
        &self,
        queue: &wgpu::Queue,
        atlas: &AtlasArray,
        rect: PendingClearRect,
    ) -> bool {
        atlas.clear_region(queue, clear_rect_region(rect))
    }

    /// Write one already-rasterized glyph pixmap into its slot, answering
    /// whether the array took it.
    ///
    /// The slot's extent is what is written, not the pixmap's: a pixmap whose
    /// dimensions disagree with the slot it was allocated for would be written
    /// at the wrong stride and smear across the page, so the mismatch is
    /// refused here rather than handed to the queue.
    pub fn upload_pixmap(
        &self,
        queue: &wgpu::Queue,
        atlas: &AtlasArray,
        upload: &PendingBitmapUpload,
    ) -> bool {
        let region = slot_region(upload);
        if u32::from(upload.pixmap.width()) != region.size[0]
            || u32::from(upload.pixmap.height()) != region.size[1]
        {
            return false;
        }
        atlas.write_region(queue, region, upload.pixmap.data_as_u8_slice())
    }

    /// Draw one page's lowered replay into array layer `layer`, on a command
    /// encoder of this method's own, submitted before it returns.
    ///
    /// Returns how many instances were drawn — zero for an empty page, which
    /// costs no submit at all.
    ///
    /// `timestamps` charges this pass to [`EngineSpan::Prepass`] — the atlas
    /// replay's own recording site, one own-encoder submit per dirty page (see
    /// this module's header), so several pages in one frame are several passes
    /// summed into the one span. The fresh pair is asked for only once the
    /// empty-page early return is behind us, so a page with nothing to draw
    /// never spends a query pair on a pass that was never opened.
    ///
    /// # Errors
    ///
    /// [`EngineError::AtlasError`] when the array has no layer `layer`;
    /// [`EngineError::AlphaCapacity`] when the page's coverage is past what a
    /// resource texture of this adapter's dimension can hold. Both leave the
    /// atlas exactly as it was — nothing is recorded before either is checked.
    #[expect(
        clippy::too_many_arguments,
        reason = "one page's whole draw call: device/queue, the pipeline, the \
                  array it draws into, which layer, the page's own instances, \
                  and the timestamp sink, each owned by a different caller"
    )]
    pub fn render_page(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::RenderPipeline,
        atlas: &AtlasArray,
        layer: u32,
        page: &mut AtlasPageBuffers,
        timestamps: FrameTimestamps<'_>,
    ) -> Result<u32, EngineError> {
        if page.is_empty() {
            return Ok(0);
        }
        let Some(target) = atlas.layer_view(layer) else {
            return Err(EngineError::AtlasError);
        };
        let height = super::alpha_texture_height(page.alphas.len(), self.alphas.width)?;

        if height > self.alphas.height {
            self.alphas = AtlasResourceTexture::new(
                device,
                &super::alpha_texture_descriptor(self.alphas.width, height),
            );
        }
        let alphas = &self.alphas;
        super::with_padded_alphas(&mut page.alphas, alphas.width, alphas.height, |bytes| {
            queue.write_texture(
                alphas.copy_target(),
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(super::resource_bytes_per_row(alphas.width)),
                    rows_per_image: Some(alphas.height),
                },
                alphas.extent(),
            );
        });

        let config = super::targets::atlas_layer_config(
            atlas.size(),
            self.alphas.width,
            PLACEHOLDER_PAINT_TEX_WIDTH,
        );
        queue.write_buffer(&self.config, 0, bytemuck::bytes_of(&config));

        let bytes: &[u8] = bytemuck::cast_slice(&page.instances);
        self.grow_instances(device, bytes.len() as u64);
        queue.write_buffer(&self.instances, 0, bytes);

        let groups = self.bind_groups(device, pipeline);
        let count = u32::try_from(page.instances.len()).unwrap_or(u32::MAX);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-engine atlas replay"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frust-engine atlas replay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Loaded, never cleared: the layer holds every glyph
                        // resident in it, and this pass adds one page's worth
                        // to them. A clear here would evict the whole atlas
                        // every time one glyph missed.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: timestamps.writes(EngineSpan::Prepass),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for (index, group) in groups.iter().enumerate() {
                pass.set_bind_group(index as u32, group, &[]);
            }
            pass.draw(GpuStrip::vertex_range(), GpuStrip::instance_range(0, count));
        }
        queue.submit(std::iter::once(encoder.finish()));

        Ok(count)
    }

    /// Service every drain `glyphs` has pending and replay every page it
    /// dirtied, strictly ahead of the caller's own scene pass.
    ///
    /// The order is the contract: evicted rectangles are zeroed, then bitmap
    /// pixmaps are written, then each dirty page's commands are lowered by
    /// `lower` and drawn. `lower` answers `false` for a page it declines — a
    /// command stream carrying a shape this tier has no lowering for, which is
    /// how a colour glyph goes *missing* rather than landing wrong — and the
    /// page is counted in [`AtlasRenderReport::refused`] and left undrawn.
    ///
    /// `pipeline` must be the one built from
    /// [`super::pipelines::atlas_strip_desc`]: a bind group is built against
    /// the pipeline's own derived layout, so another variant's is rejected even
    /// where the two layouts are structurally identical.
    ///
    /// `timestamps` is passed straight through to [`Self::render_page`] for
    /// each dirty page replayed — see that method's docs for the [`EngineSpan`]
    /// it charges to and why an empty page spends no query pair on it.
    #[expect(
        clippy::too_many_arguments,
        reason = "the whole replay pass's inputs: device/queue, the pipeline, \
                  the array and the glyph source it drains, the timestamp \
                  sink, and the caller's own lowering closure, each owned by \
                  a different part of the renderer"
    )]
    pub fn render_pending<L>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::RenderPipeline,
        atlas: &AtlasArray,
        glyphs: &mut GlyphAtlas,
        timestamps: FrameTimestamps<'_>,
        mut lower: L,
    ) -> AtlasRenderReport
    where
        L: FnMut(&AtlasCommandRecorder, &mut AtlasPageBuffers) -> bool,
    {
        let mut report = AtlasRenderReport::default();

        for rect in glyphs.drain_pending_clear_rects() {
            if self.clear_rect(queue, atlas, rect) {
                report.cleared = report.cleared.saturating_add(1);
            } else {
                report.refused = report.refused.saturating_add(1);
            }
        }
        for upload in glyphs.drain_pending_uploads() {
            if self.upload_pixmap(queue, atlas, &upload) {
                report.uploaded = report.uploaded.saturating_add(1);
            } else {
                report.refused = report.refused.saturating_add(1);
            }
        }

        // Taken out and put back so the closure below can hold the scratch
        // mutably while `self` records the pass it feeds.
        let mut buffers = core::mem::take(&mut self.buffers);
        glyphs.replay_pending_atlas_commands(|recorder| {
            buffers.clear();
            if !lower(recorder, &mut buffers) {
                report.refused = report.refused.saturating_add(1);
                return;
            }
            match self.render_page(
                device,
                queue,
                pipeline,
                atlas,
                recorder.page_index,
                &mut buffers,
                timestamps,
            ) {
                Ok(0) => {}
                Ok(instances) => {
                    report.pages = report.pages.saturating_add(1);
                    report.instances = report.instances.saturating_add(instances);
                }
                Err(_) => report.refused = report.refused.saturating_add(1),
            }
        });
        self.buffers = buffers;

        report
    }

    /// Grows the instance buffer if `required` bytes outgrew it.
    fn grow_instances(&mut self, device: &wgpu::Device, required: u64) {
        if self.instance_capacity >= required {
            return;
        }
        let capacity = required
            .checked_next_power_of_two()
            .unwrap_or(required)
            .max(MIN_ATLAS_INSTANCES * size_of::<GpuStrip>() as u64);
        self.instances = device.create_buffer(&instance_descriptor(capacity));
        self.instance_capacity = capacity;
    }

    /// The four bind groups an atlas pass sets, built against `pipeline`'s own
    /// derived layout.
    ///
    /// Built per pass rather than cached: a replay happens only on a frame that
    /// missed a glyph, and caching would have to be invalidated against both
    /// the coverage texture's growth and the pipeline it was derived from — two
    /// invalidation sources for a path that is not the frame path.
    fn bind_groups(
        &self,
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
    ) -> [wgpu::BindGroup; 4] {
        [
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frust-engine atlas pass resources"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.alphas.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.config.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            &self.placeholders.layer_input,
                        ),
                    },
                ],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frust-engine atlas pass images"),
                layout: &pipeline.get_bind_group_layout(1),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.placeholders.array),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.placeholders.external),
                    },
                ],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frust-engine atlas pass paints"),
                layout: &pipeline.get_bind_group_layout(2),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.placeholders.paints),
                }],
            }),
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frust-engine atlas pass gradients"),
                layout: &pipeline.get_bind_group_layout(3),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.placeholders.gradients),
                }],
            }),
        ]
    }
}

/// The instance buffer's descriptor at `size` bytes.
fn instance_descriptor(size: u64) -> wgpu::BufferDescriptor<'static> {
    wgpu::BufferDescriptor {
        label: Some("frust-engine atlas strip instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }
}

/// The atlas rectangle one of `glifo`'s pending clear rects names.
///
/// The rect is already the *padded* region — the allocation an evicted glyph
/// gave back, transparent border included — which is exactly the rectangle that
/// has to be zeroed: leaving the padding behind would let a later, smaller
/// tenant's `Extend::Pad` sampling read the previous glyph's edge texels.
#[must_use]
pub fn clear_rect_region(rect: PendingClearRect) -> AtlasRegion {
    AtlasRegion {
        layer: rect.page_index,
        offset: [u32::from(rect.x), u32::from(rect.y)],
        size: [u32::from(rect.width), u32::from(rect.height)],
    }
}

/// The atlas rectangle one pending bitmap upload's slot occupies.
///
/// The slot's own extent, not the padded allocation's: the padding around a
/// glyph is transparent by construction (a fresh page starts zeroed, an evicted
/// one is zeroed by its clear rect), so an upload writes the glyph and leaves
/// the border alone.
#[must_use]
pub fn slot_region(upload: &PendingBitmapUpload) -> AtlasRegion {
    let slot = upload.atlas_slot;
    AtlasRegion {
        layer: slot.page_index,
        offset: [u32::from(slot.x), u32::from(slot.y)],
        size: [u32::from(slot.width), u32::from(slot.height)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_common::kurbo::Point;

    fn region(layer: u32, offset: [u32; 2], size: [u32; 2]) -> AtlasRegion {
        AtlasRegion {
            layer,
            offset,
            size,
        }
    }

    #[test]
    fn the_atlas_is_a_sampled_copyable_renderable_rgba8_array() {
        let descriptor = atlas_texture_descriptor(1024, 1024, 4);

        assert_eq!(descriptor.format, ATLAS_FORMAT);
        assert_eq!(descriptor.dimension, wgpu::TextureDimension::D2);
        assert_eq!(descriptor.size.depth_or_array_layers, 4);
        assert_eq!(descriptor.mip_level_count, 1);
        for usage in [
            wgpu::TextureUsages::TEXTURE_BINDING,
            wgpu::TextureUsages::COPY_DST,
            wgpu::TextureUsages::COPY_SRC,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ] {
            assert!(descriptor.usage.contains(usage));
        }
    }

    #[test]
    fn a_zero_extent_descriptor_is_raised_to_a_creatable_one() {
        let descriptor = atlas_texture_descriptor(0, 0, 0);
        assert_eq!(descriptor.size.width, 1);
        assert_eq!(descriptor.size.height, 1);
        assert_eq!(descriptor.size.depth_or_array_layers, 2);
    }

    /// The layer floor is two, not one — a fix for wgpu-hal 30.0.1's GLES
    /// backend, not an arbitrary minimum.
    ///
    /// `get_info_from_desc` (wgpu-hal `src/gles/mod.rs:513-530`) chooses the GL
    /// target from `TextureDescriptor::size.depth_or_array_layers` alone —
    /// `(false, 1) => TEXTURE_2D`, never consulting the view dimension a caller
    /// later binds the texture through. A one-layer atlas array descriptor
    /// therefore creates a plain `GL_TEXTURE_2D`, while
    /// `crates/frust-engine/shaders/strip.wgsl` always samples this texture as
    /// `texture_2d_array<f32>` (`sampler2DArray` once naga lowers it to GLSL).
    /// Target and sampler disagreeing makes the texture incomplete per GLES
    /// 3.0 §3.8.2, so every sample reads `(0, 0, 0, 1)`: a solid box in place
    /// of a glyph, a solid black rect in place of an image — reproduced on
    /// Chrome's WebGL2 backend in `examples/web-spike` (spike w0-07) and fixed
    /// by this floor. Tracked upstream as wgpu issues #1614 and #1574; a later
    /// tidy-up must not "simplify" this back to `max(1)` without wgpu-hal
    /// fixing the heuristic first (see `docs/LIMITATIONS.md`).
    #[test]
    fn the_layer_floor_is_two_because_wgpu_hal_gles_ignores_the_view_dimension() {
        assert_eq!(
            atlas_texture_descriptor(64, 64, 0)
                .size
                .depth_or_array_layers,
            2
        );
        assert_eq!(
            atlas_texture_descriptor(64, 64, 1)
                .size
                .depth_or_array_layers,
            2
        );
        assert_eq!(
            atlas_texture_descriptor(64, 64, 3)
                .size
                .depth_or_array_layers,
            3,
            "a request already past the floor is not clamped down to it"
        );
    }

    #[test]
    fn the_view_the_shader_binds_is_a_layered_one() {
        assert_eq!(
            atlas_view_descriptor().dimension,
            Some(wgpu::TextureViewDimension::D2Array)
        );
    }

    #[test]
    fn image_params_round_trip_through_the_shaders_own_field_widths() {
        let packed = pack_image_params(1, 2, 3, 200, false);

        assert_eq!(packed & 0b11, 1, "quality");
        assert_eq!((packed >> 2) & 0b11, 2, "extend_x");
        assert_eq!((packed >> 4) & 0b11, 3, "extend_y");
        assert_eq!((packed >> 6) & 0xFF, 200, "atlas index");
        assert_eq!((packed >> 14) & 1, 0, "source kind");

        assert_eq!(pack_image_params(0, 0, 0, 0, true) >> 14 & 1, 1);
    }

    #[test]
    fn size_and_offset_pack_with_the_first_component_high() {
        assert_eq!(pack_image_size(0x1234, 0x5678), 0x1234_5678);
        assert_eq!(pack_image_offset(0x00FF, 0xAB00), 0x00FF_AB00);
    }

    #[test]
    fn an_absent_tint_is_the_identity_multiply() {
        let (color, mode) = pack_tint(None);
        assert_eq!(color, u32::MAX);
        assert_eq!(mode, TintMode::Multiply.as_u32());
    }

    #[test]
    fn natural_to_dest_lands_the_natural_corners_on_the_dest_corners() {
        let dest = Rect::new(5.0, 6.0, 45.0, 46.0);
        let transform = natural_to_dest(Affine::IDENTITY, (2, 2), dest).expect("non-degenerate");

        assert_eq!(transform * Point::new(0.0, 0.0), Point::new(5.0, 6.0));
        assert_eq!(transform * Point::new(2.0, 2.0), Point::new(45.0, 46.0));
    }

    #[test]
    fn natural_to_dest_composes_the_widgets_own_transform_outermost() {
        let dest = Rect::new(0.0, 0.0, 4.0, 4.0);
        let transform =
            natural_to_dest(Affine::translate((10.0, 20.0)), (2, 2), dest).expect("non-degenerate");

        assert_eq!(transform * Point::new(0.0, 0.0), Point::new(10.0, 20.0));
    }

    #[test]
    fn natural_to_dest_refuses_a_degenerate_natural_size() {
        let dest = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(natural_to_dest(Affine::IDENTITY, (0, 4), dest).is_none());
        assert!(natural_to_dest(Affine::IDENTITY, (4, 0), dest).is_none());
    }

    #[test]
    fn advances_are_the_linear_part_only() {
        let transform = Affine::new([2.0, 3.0, 4.0, 5.0, 100.0, 200.0]);
        let (x_advance, y_advance) = x_y_advances(transform);

        assert_eq!(x_advance, Vec2::new(2.0, 3.0));
        assert_eq!(y_advance, Vec2::new(4.0, 5.0));
    }

    #[test]
    fn a_layer_target_names_exactly_one_layer_as_a_plain_2d_view() {
        let descriptor = atlas_layer_view_descriptor(3);

        assert_eq!(descriptor.dimension, Some(wgpu::TextureViewDimension::D2));
        assert_eq!(descriptor.base_array_layer, 3);
        assert_eq!(
            descriptor.array_layer_count,
            Some(1),
            "a colour attachment must name exactly one layer"
        );
        assert_eq!(descriptor.base_mip_level, 0);
        // Distinct from the sampling view in the one axis that matters: the
        // same texture is bound array-wide for reading and 2D for writing.
        assert_ne!(descriptor.dimension, atlas_view_descriptor().dimension);
    }

    #[test]
    fn a_clear_rect_becomes_its_whole_padded_rectangle() {
        let region = clear_rect_region(PendingClearRect {
            page_index: 2,
            x: 40,
            y: 8,
            width: 18,
            height: 22,
        });

        assert_eq!(region.layer, 2);
        assert_eq!(region.offset, [40, 8]);
        assert_eq!(region.size, [18, 22]);
        assert_eq!(region.byte_len(), 18 * 22 * ATLAS_FORMAT_BYTES as usize);
    }

    /// One solid-painted run's instances, checked against the shape the shader
    /// reads them at.
    ///
    /// The run carries a trailing sentinel: a strip's width is the *coverage*
    /// distance to the strip after it (`Strip::width_to`), not the distance
    /// between their x positions, which is why the generator emits a sentinel
    /// and why a caller that trimmed it would silently lose the last span.
    #[test]
    fn a_solid_run_expands_to_alpha_sampled_spans_carrying_one_colour() {
        let run = [
            Strip::new(4, 0, 0, false),
            Strip::new(20, 0, 32, false),
            Strip::new(64, 0, 80, false),
        ];
        let mut out = Vec::new();

        let pushed = push_solid_strips(&run, 0xDEAD_BEEF, 7, &mut out);

        assert_eq!(pushed, 2, "two pairs, each contributing one span");
        assert_eq!(out.len(), 2);
        for instance in &out {
            assert_eq!(instance.payload, 0xDEAD_BEEF, "a solid colour is per draw");
            assert_eq!(instance.depth_index, 7);
            assert!(!instance.is_rect());
            assert_eq!(
                instance.paint(),
                pack_paint_descriptor(PaintType::Solid, 0),
                "a solid instance indexes no encoded-paint record"
            );
            assert_eq!(
                instance.dense_width_or_rect_height, instance.width,
                "every column of a replayed glyph samples coverage"
            );
        }
        assert_eq!(out[0].x, 4);
        assert_eq!(
            out[0].width, 8,
            "32 coverage bytes at 4 per column is 8 pixels"
        );
        assert_eq!(out[0].col_idx_or_rect_frac, 0);
        assert_eq!(out[1].x, 20);
        assert_eq!(out[1].width, 12);
        assert_eq!(
            out[1].col_idx_or_rect_frac, 8,
            "the second span starts at the column its coverage does"
        );
    }

    #[test]
    fn a_winding_gap_between_two_strips_is_filled_solid() {
        // The second strip carries the fill-gap flag on the same row, so the
        // span between them is inside the glyph and gets a coverage-free fill.
        let run = [
            Strip::new(0, 0, 0, false),
            Strip::new(32, 0, 16, true),
            Strip::new(48, 0, 32, false),
        ];
        let mut out = Vec::new();

        let pushed = push_solid_strips(&run, 0x11, 0, &mut out);

        assert_eq!(pushed, 3, "two spans plus the gap between them");
        let gap = out
            .iter()
            .find(|instance| instance.dense_width_or_rect_height == 0)
            .copied();
        let gap = match gap {
            Some(gap) => gap,
            None => unreachable!("the flagged pair fills its gap"),
        };
        assert_eq!(gap.x, 4, "the gap starts where the first strip ends");
        assert_eq!(gap.width, 28);
        assert_eq!(gap.payload, 0x11, "the fill takes the run's own colour");
        assert_eq!(gap.col_idx_or_rect_frac, 0, "a gap samples no coverage");
    }

    #[test]
    fn a_run_with_no_pair_in_it_draws_nothing() {
        let mut out = Vec::new();
        assert_eq!(push_solid_strips(&[], 0, 0, &mut out), 0);
        assert_eq!(
            push_solid_strips(&[Strip::new(0, 0, 0, false)], 0, 0, &mut out),
            0,
            "a lone sentinel is not a span"
        );
        assert!(out.is_empty());
    }

    #[test]
    fn an_emptied_page_keeps_its_capacity_for_the_next_one() {
        let mut buffers = AtlasPageBuffers::default();
        assert!(buffers.is_empty());

        buffers.instances.push(GpuStrip::solid_fill(
            0,
            0,
            8,
            StripDraw {
                payload: 0,
                paint: 0,
                depth_index: 0,
            },
        ));
        buffers.alphas.extend_from_slice(&[1, 2, 3, 4]);
        assert!(!buffers.is_empty());

        let instances = buffers.instances.capacity();
        let alphas = buffers.alphas.capacity();
        buffers.clear();

        assert!(buffers.is_empty());
        assert!(buffers.alphas.is_empty());
        assert_eq!(buffers.instances.capacity(), instances);
        assert_eq!(buffers.alphas.capacity(), alphas);
    }

    #[test]
    fn a_report_that_serviced_nothing_reads_as_empty() {
        let mut report = AtlasRenderReport::default();
        assert!(report.is_empty());

        report.refused = 1;
        assert!(
            !report.is_empty(),
            "a refusal is work that happened, not an idle frame"
        );
    }

    #[test]
    fn the_stand_in_paint_texture_width_is_reconstructable_by_the_shader() {
        // The shader rebuilds the width as `1 << bits`, so a stand-in that was
        // not a power of two would make the config disagree with the texture
        // actually bound.
        assert!(PLACEHOLDER_PAINT_TEX_WIDTH.is_power_of_two());
        assert_eq!(
            super::super::config::tex_width_bits(PLACEHOLDER_PAINT_TEX_WIDTH),
            0
        );
    }

    #[test]
    fn a_regions_byte_footprint_and_stride_agree_with_rgba8() {
        let populated = region(0, [4, 8], [16, 32]);
        assert_eq!(populated.bytes_per_row(), 64);
        assert_eq!(populated.byte_len(), 64 * 32);
        assert_eq!(region_byte_len(populated), populated.byte_len());
        assert!(!populated.is_empty());
        assert!(region(0, [0, 0], [0, 4]).is_empty());
    }
}
