//! The GPU-side data contract: sizes, field offsets, vertex layout, strip
//! conversion, alpha packing and encoded-paint serialization.
//!
//! These layouts are read by hand-written WGSL, so nothing here checks that
//! the code compiles — it checks that the bytes land where the shaders expect
//! them. A silent layout change is a mis-render, not a build failure, which
//! is why the assertions are on offsets and byte sequences rather than on
//! behavior.

use std::mem::{align_of, offset_of, size_of};

use frust_engine::EngineError;
use frust_engine::gpu::{
    self, GpuBlurredRoundedRect, GpuConfig, GpuEncodedImage, GpuEncodedPaint, GpuLinearGradient,
    GpuRadialGradient, GpuStrip, GpuSweepGradient, StripDraw, paint_texture, strips,
};
use vello_common::strip::Strip;
use vello_common::tile::Tile;

/// A strip's width is defined by how far the next strip's alpha column is,
/// four alpha values per pixel column.
fn next_strip(x: u16, y: u16, alpha_idx: u32, width: u16) -> Strip {
    Strip::new(
        x,
        y,
        alpha_idx + u32::from(width) * u32::from(Tile::HEIGHT),
        false,
    )
}

fn draw() -> StripDraw {
    StripDraw {
        payload: 0x0000_00ff,
        paint: 0x1234_5678,
        depth_index: 7,
    }
}

#[test]
fn gpu_strip_is_24_bytes_with_the_shader_field_order() {
    assert_eq!(size_of::<GpuStrip>(), 24);
    assert_eq!(offset_of!(GpuStrip, x), 0);
    assert_eq!(offset_of!(GpuStrip, y), 2);
    assert_eq!(offset_of!(GpuStrip, width), 4);
    assert_eq!(offset_of!(GpuStrip, dense_width_or_rect_height), 6);
    assert_eq!(offset_of!(GpuStrip, col_idx_or_rect_frac), 8);
    assert_eq!(offset_of!(GpuStrip, payload), 12);
    assert_eq!(offset_of!(GpuStrip, paint_and_rect_flag), 16);
    assert_eq!(offset_of!(GpuStrip, depth_index), 20);
}

#[test]
fn strip_vertex_layout_is_six_instance_stepped_uint32_attributes() {
    let layout = GpuStrip::vertex_layout();

    assert_eq!(layout.array_stride, size_of::<GpuStrip>() as u64);
    assert_eq!(layout.step_mode, wgpu::VertexStepMode::Instance);
    assert_eq!(layout.attributes.len(), 6);

    for (index, attribute) in layout.attributes.iter().enumerate() {
        assert_eq!(attribute.shader_location, index as u32);
        assert_eq!(attribute.format, wgpu::VertexFormat::Uint32);
        assert_eq!(attribute.offset, (index * size_of::<u32>()) as u64);
    }

    // The borrowed wgpu view is what a pipeline descriptor actually takes.
    let wgpu_layout = layout.as_wgpu();
    assert_eq!(wgpu_layout.array_stride, 24);
    assert_eq!(wgpu_layout.attributes.len(), 6);
}

#[test]
fn a_strip_draw_is_one_four_vertex_quad_per_instance() {
    assert_eq!(GpuStrip::QUAD_VERTICES, 4);
    assert_eq!(GpuStrip::vertex_range(), 0..4);
    assert_eq!(GpuStrip::instance_range(3, 5), 3..8);
    // A count that would overflow saturates rather than panicking on a frame
    // path.
    assert_eq!(GpuStrip::instance_range(u32::MAX, 1), u32::MAX..u32::MAX);
}

#[test]
fn tile_height_is_four() {
    assert_eq!(Tile::HEIGHT, 4);
}

#[test]
fn a_known_strip_converts_to_the_expected_instance_bytes() {
    let strip = Strip::new(8, 4, 64, false);
    let next = next_strip(64, 4, 64, 16);
    assert_eq!(strip.width_to(&next), 16);

    let gpu_strip = GpuStrip::from_strip_pair(&strip, &next, draw());

    assert_eq!(gpu_strip.x, 8);
    assert_eq!(gpu_strip.y, 4);
    assert_eq!(gpu_strip.width, 16);
    assert_eq!(gpu_strip.dense_width_or_rect_height, 16);
    assert_eq!(gpu_strip.col_idx_or_rect_frac, 64);
    assert_eq!(gpu_strip.payload, 0x0000_00ff);
    assert_eq!(gpu_strip.paint_and_rect_flag, 0x1234_5678);
    assert_eq!(gpu_strip.depth_index, 7);
    assert!(!gpu_strip.is_rect());

    let mut expected = Vec::new();
    expected.extend_from_slice(&8_u16.to_ne_bytes());
    expected.extend_from_slice(&4_u16.to_ne_bytes());
    expected.extend_from_slice(&16_u16.to_ne_bytes());
    expected.extend_from_slice(&16_u16.to_ne_bytes());
    expected.extend_from_slice(&64_u32.to_ne_bytes());
    expected.extend_from_slice(&0x0000_00ff_u32.to_ne_bytes());
    expected.extend_from_slice(&0x1234_5678_u32.to_ne_bytes());
    expected.extend_from_slice(&7_u32.to_ne_bytes());

    assert_eq!(bytemuck::bytes_of(&gpu_strip), expected.as_slice());
}

#[test]
fn the_fill_gap_flag_emits_a_solid_span_between_two_strips() {
    let strip = Strip::new(8, 4, 64, false);
    // 16 pixels wide, so the strip ends at x = 24 and the gap runs to x = 40.
    let next = Strip::new(40, 4, 64 + 16 * u32::from(Tile::HEIGHT), true);

    let gap = GpuStrip::gap_fill(&strip, &next, draw()).expect("the gap is filled");

    assert_eq!(gap.x, 24);
    assert_eq!(gap.y, 4);
    assert_eq!(gap.width, 16);
    // A solid span samples no coverage.
    assert_eq!(gap.dense_width_or_rect_height, 0);
    assert_eq!(gap.col_idx_or_rect_frac, 0);
    assert_eq!(gap.paint_and_rect_flag, 0x1234_5678);
}

#[test]
fn a_gap_is_not_filled_without_the_flag_or_across_strip_rows() {
    let strip = Strip::new(8, 4, 64, false);
    let unflagged = next_strip(40, 4, 64, 16);
    assert!(GpuStrip::gap_fill(&strip, &unflagged, draw()).is_none());

    let other_row = Strip::new(40, 8, 64 + 16 * u32::from(Tile::HEIGHT), true);
    assert!(GpuStrip::gap_fill(&strip, &other_row, draw()).is_none());

    let adjacent = Strip::new(24, 4, 64 + 16 * u32::from(Tile::HEIGHT), true);
    assert!(GpuStrip::gap_fill(&strip, &adjacent, draw()).is_none());
}

#[test]
fn a_rect_instance_carries_the_rect_flag_in_bit_31() {
    let rect = GpuStrip::from_rect(2, 4, 30, 12, 0x00ff_0000, draw());

    assert!(rect.is_rect());
    assert_eq!(
        rect.paint_and_rect_flag,
        0x1234_5678 | strips::RECT_STRIP_FLAG
    );
    assert_eq!(rect.paint(), 0x1234_5678);
    // Height and coverage fraction take over the two overloaded fields.
    assert_eq!(rect.dense_width_or_rect_height, 12);
    assert_eq!(rect.col_idx_or_rect_frac, 0x00ff_0000);

    let strip = GpuStrip::solid_fill(2, 4, 30, draw());
    assert!(!strip.is_rect());
    assert_eq!(strip.paint(), strip.paint_and_rect_flag);
}

#[test]
fn gpu_config_is_a_32_byte_uniform_in_the_shader_field_order() {
    assert_eq!(size_of::<GpuConfig>(), 32);
    assert_eq!(align_of::<GpuConfig>(), 16);
    assert_eq!(GpuConfig::SIZE, 32);

    assert_eq!(offset_of!(GpuConfig, width), 0);
    assert_eq!(offset_of!(GpuConfig, height), 4);
    assert_eq!(offset_of!(GpuConfig, strip_height), 8);
    assert_eq!(offset_of!(GpuConfig, alphas_tex_width_bits), 12);
    assert_eq!(offset_of!(GpuConfig, encoded_paints_tex_width_bits), 16);
    assert_eq!(offset_of!(GpuConfig, strip_offset_x), 20);
    assert_eq!(offset_of!(GpuConfig, strip_offset_y), 24);
    assert_eq!(offset_of!(GpuConfig, negate_ndc), 28);
}

#[test]
fn gpu_config_precomputes_log2_texture_widths() {
    let config = GpuConfig::new(800, 600, 4096, 2048);

    assert_eq!(config.width, 800);
    assert_eq!(config.height, 600);
    assert_eq!(config.strip_height, u32::from(Tile::HEIGHT));
    assert_eq!(config.alphas_tex_width_bits, 12);
    assert_eq!(config.encoded_paints_tex_width_bits, 11);
    assert_eq!(1 << config.alphas_tex_width_bits, 4096);
    assert_eq!(config.strip_offset_x, 0);
    assert_eq!(config.strip_offset_y, 0);
    assert_eq!(config.negate_ndc, 0);

    let offset = config.with_strip_offset(-3, 9).with_negate_ndc(true);
    assert_eq!(offset.strip_offset_x, -3);
    assert_eq!(offset.strip_offset_y, 9);
    assert_eq!(offset.negate_ndc, 1);

    assert_eq!(gpu::tex_width_bits(1), 0);
    assert_eq!(gpu::tex_width_bits(2048), 11);

    // The uniform is uploadable as plain bytes.
    assert_eq!(bytemuck::bytes_of(&config).len(), 32);
}

#[test]
fn a_resource_texture_is_a_sampled_uploadable_rgba32uint_target() {
    let descriptor = gpu::alpha_texture_descriptor(4096, 2);

    assert_eq!(descriptor.format, wgpu::TextureFormat::Rgba32Uint);
    assert_eq!(
        descriptor.usage,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST
    );
    assert_eq!(descriptor.dimension, wgpu::TextureDimension::D2);
    assert_eq!(descriptor.mip_level_count, 1);
    assert_eq!(descriptor.sample_count, 1);
    assert_eq!(descriptor.size.width, 4096);
    assert_eq!(descriptor.size.height, 2);
    assert_eq!(descriptor.size.depth_or_array_layers, 1);

    // 16 bytes per texel, expressed as the shift the shader also uses.
    assert_eq!(gpu::TEXEL_BYTES, 16);
    assert_eq!(gpu::ALPHAS_PER_TEXEL, 16);
    assert_eq!(gpu::ALPHAS_PER_CHANNEL, 4);
    assert_eq!(gpu::resource_bytes_per_row(4096), 4096 << 4);
    assert_eq!(gpu::resource_texture_bytes(4096, 2), 4096 * 2 * 16);
}

#[test]
fn the_alpha_texture_starts_at_one_row_and_grows_by_sixteen_alphas_per_texel() {
    let dim = 4096;
    let per_row = u64::from(dim) * 16;

    assert_eq!(gpu::alpha_texture_height(0, dim).unwrap(), 1);
    assert_eq!(gpu::alpha_texture_height(1, dim).unwrap(), 1);
    assert_eq!(
        gpu::alpha_texture_height(per_row as usize, dim).unwrap(),
        1,
        "a full row still fits in one row"
    );
    assert_eq!(
        gpu::alpha_texture_height(per_row as usize + 1, dim).unwrap(),
        2
    );

    // Growth only: a frame needing less keeps the taller texture.
    assert_eq!(gpu::grow_alpha_texture_height(1, 1, dim).unwrap(), None);
    assert_eq!(
        gpu::grow_alpha_texture_height(1, per_row as usize + 1, dim).unwrap(),
        Some(2)
    );
    assert_eq!(
        gpu::grow_alpha_texture_height(4, per_row as usize + 1, dim).unwrap(),
        None
    );
}

#[test]
fn alpha_coverage_past_the_texture_ceiling_is_an_error_not_a_panic() {
    let dim = 4096;
    // One alpha past a full square texture of `dim` x `dim` texels.
    let over_capacity = (dim as usize) * (dim as usize) * 16 + 1;

    assert!(matches!(
        gpu::alpha_texture_height(over_capacity, dim),
        Err(EngineError::AlphaCapacity)
    ));
    assert!(matches!(
        gpu::grow_alpha_texture_height(1, over_capacity, dim),
        Err(EngineError::AlphaCapacity)
    ));
}

#[test]
fn alpha_packing_round_trips_across_texel_and_row_boundaries() {
    // A two-texel-wide texture holds 32 alphas per row, so 37 alphas cross
    // both a texel boundary (at 16) and a row boundary (at 32).
    let width = 2;
    let mut alphas: Vec<u8> = (0..37_u32).map(|i| (i * 7 + 1) as u8).collect();
    let original = alphas.clone();

    let height = gpu::alpha_texture_height(alphas.len(), width).unwrap();
    assert_eq!(height, 2);

    // Addressing: 16 alphas per texel, 4 per channel, texels row-major.
    assert_eq!(
        gpu::alpha_address(0, width),
        gpu::AlphaAddress {
            x: 0,
            y: 0,
            channel: 0,
            byte: 0
        }
    );
    assert_eq!(
        gpu::alpha_address(16, width),
        gpu::AlphaAddress {
            x: 1,
            y: 0,
            channel: 0,
            byte: 0
        },
        "the 17th alpha starts the next texel"
    );
    assert_eq!(
        gpu::alpha_address(32, width),
        gpu::AlphaAddress {
            x: 0,
            y: 1,
            channel: 0,
            byte: 0
        },
        "the 33rd alpha starts the next row"
    );
    assert_eq!(
        gpu::alpha_address(37, width),
        gpu::AlphaAddress {
            x: 0,
            y: 1,
            channel: 1,
            byte: 1
        }
    );

    gpu::with_padded_alphas(&mut alphas, width, height, |padded| {
        assert_eq!(
            padded.len(),
            gpu::resource_texture_bytes(width, height) as usize,
            "an upload covers the whole texture extent"
        );

        for (index, expected) in original.iter().enumerate() {
            let address = gpu::alpha_address(index as u32, width);
            let offset = address.byte_offset(width) as usize;
            assert_eq!(offset, index, "alpha values are laid out linearly");
            assert_eq!(padded[offset], *expected);
        }

        assert!(
            padded[original.len()..].iter().all(|byte| *byte == 0),
            "the pad is zeroed"
        );
    });

    assert_eq!(
        alphas, original,
        "the buffer is truncated back after upload"
    );
}

#[test]
fn encoded_paint_records_are_sixteen_byte_aligned_and_fixed_size() {
    assert_eq!(size_of::<GpuEncodedImage>(), 48);
    assert_eq!(size_of::<GpuLinearGradient>(), 32);
    assert_eq!(size_of::<GpuRadialGradient>(), 64);
    assert_eq!(size_of::<GpuSweepGradient>(), 48);
    assert_eq!(size_of::<GpuBlurredRoundedRect>(), 80);

    assert_eq!(align_of::<GpuEncodedImage>(), 16);
    assert_eq!(align_of::<GpuLinearGradient>(), 16);
    assert_eq!(align_of::<GpuRadialGradient>(), 16);
    assert_eq!(align_of::<GpuSweepGradient>(), 16);
    assert_eq!(align_of::<GpuBlurredRoundedRect>(), 16);
}

fn image_paint() -> GpuEncodedPaint {
    GpuEncodedPaint::Image(GpuEncodedImage {
        image_params: 0x0000_0041,
        image_size: 0x0010_0020,
        image_offset: 0x0002_0003,
        transform: [1.0, 0.0, 0.0, 1.0, 4.0, 5.0],
        tint: 0xffff_ffff,
        tint_mode: 1,
        image_padding: 2,
    })
}

fn linear_paint() -> GpuEncodedPaint {
    GpuEncodedPaint::LinearGradient(GpuLinearGradient {
        texture_width_and_extend_mode: paint_texture::pack_texture_width_and_extend_mode(256, 1),
        gradient_start: 64,
        transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    })
}

#[test]
fn encoded_paints_serialize_back_to_back_at_texel_boundaries() {
    let paints = [image_paint(), linear_paint()];

    assert_eq!(paints[0].byte_len(), 48);
    assert_eq!(paints[0].texel_len(), 3);
    assert_eq!(paints[1].byte_len(), 32);
    assert_eq!(paints[1].texel_len(), 2);
    assert_eq!(GpuEncodedPaint::serialized_len(&paints), 80);
    // A record starts at the texel the previous ones ended on.
    assert_eq!(GpuEncodedPaint::texel_offsets(&paints), vec![0, 3]);

    // A padded upload buffer, as the texture would be uploaded from.
    let mut buffer = vec![0_u8; gpu::resource_texture_bytes(4, 2) as usize];
    let written = GpuEncodedPaint::serialize_to_buffer(&paints, &mut buffer).unwrap();

    assert_eq!(written, 80);
    assert_eq!(&buffer[..48], paints[0].as_bytes());
    assert_eq!(&buffer[48..80], paints[1].as_bytes());
    assert!(buffer[80..].iter().all(|byte| *byte == 0));
}

#[test]
fn serializing_into_too_small_a_buffer_is_an_error_not_a_partial_write() {
    let paints = [image_paint(), linear_paint()];
    let mut buffer = vec![0_u8; 64];

    assert!(matches!(
        GpuEncodedPaint::serialize_to_buffer(&paints, &mut buffer),
        Err(EngineError::AtlasError)
    ));
    assert!(
        buffer.iter().all(|byte| *byte == 0),
        "nothing is written when the buffer cannot hold every record"
    );
}

#[test]
fn the_encoded_paint_texture_grows_by_texels_and_refuses_to_exceed_the_ceiling() {
    assert_eq!(
        paint_texture::encoded_paints_texture_height(0, 4).unwrap(),
        1
    );
    assert_eq!(
        paint_texture::encoded_paints_texture_height(4, 4).unwrap(),
        1
    );
    assert_eq!(
        paint_texture::encoded_paints_texture_height(5, 4).unwrap(),
        2
    );

    assert_eq!(
        paint_texture::grow_encoded_paints_texture_height(1, 5, 4).unwrap(),
        Some(2)
    );
    assert_eq!(
        paint_texture::grow_encoded_paints_texture_height(2, 5, 4).unwrap(),
        None
    );

    assert!(matches!(
        paint_texture::encoded_paints_texture_height(4 * 4 + 1, 4),
        Err(EngineError::PaintCapacity)
    ));

    let descriptor = paint_texture::encoded_paints_texture_descriptor(2048, 3);
    assert_eq!(descriptor.format, wgpu::TextureFormat::Rgba32Uint);
    assert_eq!(gpu::resource_bytes_per_row(2048), 2048 << 4);
}

#[test]
fn gradient_paint_fields_pack_and_unpack() {
    let packed = paint_texture::pack_texture_width_and_extend_mode(256, 2);
    assert_eq!(
        paint_texture::unpack_texture_width_and_extend_mode(packed),
        (256, 2)
    );

    let pad = paint_texture::pack_texture_width_and_extend_mode(1024, 0);
    assert_eq!(
        paint_texture::unpack_texture_width_and_extend_mode(pad),
        (1024, 0)
    );

    assert_eq!(paint_texture::pack_kind_and_f_is_swapped(0, false), 0);
    assert_eq!(paint_texture::pack_kind_and_f_is_swapped(2, false), 2);
    assert_eq!(paint_texture::pack_kind_and_f_is_swapped(1, true), 0b101);
}
