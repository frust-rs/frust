//! GPU tests for [`frust_gpu::CommandBuffer`]'s render-pass seam and
//! [`frust_gpu::HeadlessTarget`]'s clear/readback round trip. `#[ignore]`d —
//! run manually on hardware with a GPU:
//!
//! ```text
//! cargo test -p frust-gpu -- --ignored
//! ```
//!
//! On a multi-adapter host, pin the adapter the same way any other headless
//! run does (`WGPU_BACKEND=vulkan WGPU_ADAPTER_NAME=T400`) — adapter
//! selection goes through the environment-aware initializer
//! [`frust_gpu::RenderContext`] itself uses, so the knob is honoured.

use frust_gpu::{ColorAttachment, CommandBuffer, HeadlessTarget, RenderContext, RenderTarget};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Clears `target` to `color` through one [`CommandBuffer`] and reads the
/// pixels back.
fn clear_and_read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &HeadlessTarget,
    color: wgpu::Color,
) -> Vec<u8> {
    let mut command_buffer = CommandBuffer::new(device, Some("frust-gpu headless test clear"));
    {
        let render_target = RenderTarget {
            color: vec![ColorAttachment {
                view: target.view(),
                load: wgpu::LoadOp::Clear(color),
                store: wgpu::StoreOp::Store,
            }],
            depth: None,
        };
        // Begins and immediately ends the pass — a clear needs no draw call,
        // and ending it (by dropping it here) is what the borrowing contract
        // on `CommandBuffer` requires before the buffer can be finished.
        let _pass = command_buffer.render_pass(&render_target, Some("clear pass"));
    }
    queue.submit([command_buffer.finish()]);
    target.read_back(device, queue)
}

/// Rounds an `sRGB`-style 0.0..=1.0 clear-color channel to the `u8` a
/// `Rgba8Unorm` readback must report it as.
fn channel_u8(value: f64) -> u8 {
    (value * 255.0).round() as u8
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn clear_through_one_borrowed_encoder_reads_back_exactly() {
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");

        // 97x61: neither dimension is a multiple of wgpu's copy-row
        // alignment, so a stride/padding bug in the readback is visible
        // rather than accidentally masked by a convenient size.
        let width = 97;
        let height = 61;
        let target = HeadlessTarget::new(&handle.device, width, height, FORMAT);
        assert_eq!(target.width(), width);
        assert_eq!(target.height(), height);
        assert_eq!(target.format(), FORMAT);

        // Chosen as exact 8-bit fractions (64/255, 128/255, 192/255) rather
        // than round numbers like 0.5, whose `* 255` sits exactly on a
        // rounding tie and can land differently in this test's own `f64`
        // arithmetic than in the driver's fixed-point conversion.
        let color = wgpu::Color {
            r: 64.0 / 255.0,
            g: 128.0 / 255.0,
            b: 192.0 / 255.0,
            a: 1.0,
        };
        let pixels = clear_and_read_back(&handle.device, &handle.queue, &target, color);

        assert_eq!(pixels.len(), (width * height * 4) as usize);
        let expected = [
            channel_u8(color.r),
            channel_u8(color.g),
            channel_u8(color.b),
            channel_u8(color.a),
        ];
        for y in 0..height {
            for x in 0..width {
                let at = ((y * width + x) * 4) as usize;
                assert_eq!(
                    &pixels[at..at + 4],
                    &expected[..],
                    "pixel ({x}, {y}) did not read back the cleared colour"
                );
            }
        }
    });
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn two_headless_targets_on_one_context_render_in_one_submit() {
    // E12: desktop renders one window today, but the engine must not assume
    // it — two independent render targets, recorded and submitted through a
    // single command buffer on one shared `RenderContext`, must come back with
    // their own distinct contents.
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");

        let width = 32;
        let height = 24;
        let target_a = HeadlessTarget::new(&handle.device, width, height, FORMAT);
        let target_b = HeadlessTarget::new(&handle.device, width, height, FORMAT);

        let color_a = wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let color_b = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 1.0,
            a: 1.0,
        };

        let mut command_buffer = CommandBuffer::new(&handle.device, Some("two targets"));
        {
            let render_target_a = RenderTarget {
                color: vec![ColorAttachment {
                    view: target_a.view(),
                    load: wgpu::LoadOp::Clear(color_a),
                    store: wgpu::StoreOp::Store,
                }],
                depth: None,
            };
            let _pass_a = command_buffer.render_pass(&render_target_a, Some("clear a"));
        }
        {
            let render_target_b = RenderTarget {
                color: vec![ColorAttachment {
                    view: target_b.view(),
                    load: wgpu::LoadOp::Clear(color_b),
                    store: wgpu::StoreOp::Store,
                }],
                depth: None,
            };
            let _pass_b = command_buffer.render_pass(&render_target_b, Some("clear b"));
        }
        // One submit for both passes — exactly the shape the invariant on
        // `CommandBuffer` allows: any number of passes recorded through one
        // borrowed encoder, submitted once.
        handle.queue.submit([command_buffer.finish()]);

        let pixels_a = target_a.read_back(&handle.device, &handle.queue);
        let pixels_b = target_b.read_back(&handle.device, &handle.queue);

        let expected_a = [
            channel_u8(color_a.r),
            channel_u8(color_a.g),
            channel_u8(color_a.b),
            channel_u8(color_a.a),
        ];
        let expected_b = [
            channel_u8(color_b.r),
            channel_u8(color_b.g),
            channel_u8(color_b.b),
            channel_u8(color_b.a),
        ];
        assert_eq!(&pixels_a[0..4], &expected_a[..]);
        assert_eq!(&pixels_b[0..4], &expected_b[..]);
        assert_ne!(
            pixels_a, pixels_b,
            "the two targets must not share contents"
        );
    });
}
