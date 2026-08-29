//! An offscreen render target with no swapchain and no `wgpu::Surface`:
//! [`HeadlessTarget`], for tests and tooling that need to render through this
//! crate's own seams ([`crate::encoder::CommandBuffer`]) with no window.
//!
//! `HeadlessTarget` requests `RENDER_ATTACHMENT | COPY_SRC` and deliberately
//! **not** `STORAGE_BINDING` — this crate renders through ordinary render
//! passes, not compute, so a downlevel (WebGL2/GLES3.0) target never needs a
//! usage flag that profile does not offer. [`HeadlessTarget::read_back`]
//! strips wgpu's mandatory `copy_texture_to_buffer` row padding so an
//! arbitrary width — not just one whose row happens to already be aligned —
//! reads back exactly; the padding/stripping arithmetic here is written to be
//! liftable into a shared helper once `frust-render`'s own headless module
//! needs the identical calculation.

use wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as COPY_ROW_ALIGNMENT;

/// An offscreen render target: one `wgpu::Texture` plus its full-extent view,
/// created with `RENDER_ATTACHMENT | COPY_SRC` usage and no `STORAGE_BINDING`.
pub struct HeadlessTarget {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl HeadlessTarget {
    /// Creates a `width` x `height` render target in `format`.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frust-gpu headless target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            width,
            height,
            format,
            texture,
            view,
        }
    }

    /// Target width in texels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Target height in texels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The target's pixel format.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The full-extent view a render pass attaches to.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The underlying texture, e.g. as a `copy_texture_to_buffer` source.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Copies the target into a mappable buffer and returns its rows with
    /// wgpu's row padding removed — tightly packed, `width * height *
    /// bytes-per-texel` bytes, top-to-bottom, left-to-right.
    ///
    /// # Panics
    ///
    /// Panics if [`Self::format`] has no defined block size (a compressed or
    /// planar format an offscreen render target never uses), if the device
    /// poll fails, or if the buffer map fails — none of which is a condition
    /// a headless test or tool should try to recover from.
    pub fn read_back(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<u8> {
        let bytes_per_pixel = self
            .format
            .block_copy_size(None)
            .expect("a headless render target's format must have a defined block size");
        let bytes_per_row = padded_bytes_per_row(self.width, bytes_per_pixel);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-gpu headless readback"),
            size: u64::from(bytes_per_row) * u64::from(self.height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frust-gpu headless readback copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("frust-gpu headless: device poll for readback map must succeed");
        rx.recv()
            .expect("frust-gpu headless: readback map channel closed before a result arrived")
            .expect("frust-gpu headless: readback buffer map failed");

        let mapped = slice.get_mapped_range();
        let pixels = strip_row_padding(&mapped, self.width, self.height, bytes_per_pixel);
        drop(mapped);
        buffer.unmap();
        pixels
    }
}

/// The `bytes_per_row` a `copy_texture_to_buffer` of a `width`-texel row of
/// `bytes_per_pixel`-byte texels must use: the tight row length rounded up to
/// wgpu's mandatory [`COPY_ROW_ALIGNMENT`].
fn padded_bytes_per_row(width: u32, bytes_per_pixel: u32) -> u32 {
    (width * bytes_per_pixel).next_multiple_of(COPY_ROW_ALIGNMENT)
}

/// Copies the leading `width * bytes_per_pixel` bytes out of each padded row
/// of `padded`, producing tightly packed rows.
fn strip_row_padding(padded: &[u8], width: u32, height: u32, bytes_per_pixel: u32) -> Vec<u8> {
    let row = width as usize * bytes_per_pixel as usize;
    let stride = padded_bytes_per_row(width, bytes_per_pixel) as usize;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        out.extend_from_slice(&padded[start..start + row]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGBA8/BGRA8 (this crate's headless target formats): 4 bytes/texel.
    const RGBA8_BPP: u32 = 4;

    #[test]
    fn padded_row_is_the_tight_row_when_already_aligned() {
        assert_eq!(padded_bytes_per_row(64, RGBA8_BPP), 256);
        assert_eq!(padded_bytes_per_row(128, RGBA8_BPP), 512);
        assert_eq!(padded_bytes_per_row(256, RGBA8_BPP), 1024);
    }

    #[test]
    fn padded_row_rounds_an_unaligned_width_up_to_the_alignment() {
        assert_eq!(padded_bytes_per_row(1, RGBA8_BPP), 256);
        assert_eq!(padded_bytes_per_row(63, RGBA8_BPP), 256);
        assert_eq!(padded_bytes_per_row(65, RGBA8_BPP), 512);
        assert_eq!(padded_bytes_per_row(97, RGBA8_BPP), 512);
        assert_eq!(padded_bytes_per_row(129, RGBA8_BPP), 768);
        for width in 1..600u32 {
            let padded = padded_bytes_per_row(width, RGBA8_BPP);
            assert!(
                padded >= width * RGBA8_BPP,
                "padding must never truncate a row"
            );
            assert_eq!(padded % COPY_ROW_ALIGNMENT, 0, "width {width}");
            assert!(
                padded - width * RGBA8_BPP < COPY_ROW_ALIGNMENT,
                "padding must be minimal, width {width}"
            );
        }
    }

    #[test]
    fn stripping_padding_keeps_every_rows_own_pixels() {
        // 3-texel rows (12 bytes) padded to 256: each row is tagged with its
        // own index so a stride slip is visible rather than plausible.
        let width = 3;
        let height = 4;
        let stride = padded_bytes_per_row(width, RGBA8_BPP) as usize;
        let mut padded = vec![0xEE_u8; stride * height as usize];
        for y in 0..height as usize {
            for byte in 0..(width as usize * RGBA8_BPP as usize) {
                padded[y * stride + byte] = (y * 16 + byte) as u8;
            }
        }
        let stripped = strip_row_padding(&padded, width, height, RGBA8_BPP);
        assert_eq!(stripped.len(), (width * height * RGBA8_BPP) as usize);
        for y in 0..height as usize {
            for byte in 0..(width as usize * RGBA8_BPP as usize) {
                assert_eq!(
                    stripped[y * width as usize * RGBA8_BPP as usize + byte],
                    (y * 16 + byte) as u8,
                    "row {y} byte {byte}"
                );
            }
        }
        assert!(
            !stripped.contains(&0xEE),
            "no padding byte may survive the strip"
        );
    }

    #[test]
    fn stripping_an_already_aligned_width_is_a_plain_copy() {
        let width = 64;
        let height = 2;
        let padded: Vec<u8> = (0..(width * height * RGBA8_BPP))
            .map(|i| (i % 251) as u8)
            .collect();
        assert_eq!(strip_row_padding(&padded, width, height, RGBA8_BPP), padded);
    }
}
