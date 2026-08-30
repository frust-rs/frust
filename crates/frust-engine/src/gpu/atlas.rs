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
//! Clearing an evicted region writes transparent texels through the queue
//! rather than drawing a scissored pass. Both reach the same result; the queue
//! write needs no pipeline, no render pass and no bind group, and eviction is
//! a once-in-sixty-frames event whose cost is a zeroed staging buffer the size
//! of the region.

use vello_common::encode::EncodedImage;
use vello_common::kurbo::{Affine, Rect, Vec2};
use vello_common::paint::{ImageSource, Tint, TintMode};

use crate::cache::images::{ATLAS_FORMAT_BYTES, AtlasRegion, ResidentImage};

use super::GpuEncodedPaint;
use super::paint_texture::GpuEncodedImage;

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
/// `layers` is raised to at least one: a texture with zero array layers cannot
/// be created, and a renderer that has made no image resident still has to
/// have something to bind.
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
            depth_or_array_layers: layers.max(1),
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
    pub fn ensure_layers(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        layers: u32,
    ) -> bool {
        if layers <= self.layers || layers > MAX_ATLAS_INDEX + 1 {
            return false;
        }

        let grown = Self::with_layers(device, self.width, self.height, layers);
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            grown.texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: self.layers,
            },
        );

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
const fn extend_mode(extend: peniko::Extend) -> u32 {
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
        assert_eq!(descriptor.size.depth_or_array_layers, 1);
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
    fn a_regions_byte_footprint_and_stride_agree_with_rgba8() {
        let populated = region(0, [4, 8], [16, 32]);
        assert_eq!(populated.bytes_per_row(), 64);
        assert_eq!(populated.byte_len(), 64 * 32);
        assert_eq!(region_byte_len(populated), populated.byte_len());
        assert!(!populated.is_empty());
        assert!(region(0, [0, 0], [0, 4]).is_empty());
    }
}
