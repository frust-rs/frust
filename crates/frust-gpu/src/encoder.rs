//! The seam a 3D pass and the 2D engine share when recording GPU work:
//! [`CommandBuffer`], a thin wrapper around one `wgpu::CommandEncoder`.
//!
//! A renderer built on top of `frust-gpu` never owns a `wgpu::CommandEncoder`
//! directly — it is handed a [`CommandBuffer`] for the duration of one
//! recording call and must honour the borrowing discipline documented on the
//! type itself. That discipline, not a new abstraction over `wgpu::RenderPass`
//! itself, is the point of this module: [`CommandBuffer::render_pass`] builds
//! a pass from the crate's own [`RenderTarget`]/[`Attachment`](crate::texture::Attachment)
//! types so a caller never hand-assembles a `wgpu::RenderPassDescriptor`, and
//! [`CommandBuffer::encoder_mut`] is the escape hatch for whatever recording
//! this type does not model (staged uploads, a renderer's own pass shape).

use crate::texture::RenderTarget;

/// One in-flight `wgpu::CommandEncoder`, handed to a renderer under a strict
/// borrowing contract.
///
/// # Invariant
///
/// A renderer handed this encoder records only render passes and staged
/// uploads; it never submits, never begins a pass it does not end before
/// returning, holds no borrow past return; the caller may record its own
/// passes before and after and submit once. Exception: glyph-atlas uploads
/// may submit their OWN encoder so atlas content is committed before the
/// scene pass reads it (vello_hybrid render/wgpu/mod.rs:428-430).
pub struct CommandBuffer {
    encoder: wgpu::CommandEncoder,
}

impl CommandBuffer {
    /// Begins recording a fresh command buffer on `device`.
    pub fn new(device: &wgpu::Device, label: Option<&str>) -> Self {
        Self {
            encoder: device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label }),
        }
    }

    /// Begins a render pass over `target`'s color and depth attachments.
    ///
    /// The returned `wgpu::RenderPass` borrows this buffer's encoder; per the
    /// type-level invariant, a caller must end it (drop it, or call `.end()`)
    /// before recording anything else through [`Self::encoder_mut`], starting
    /// another pass, or calling [`Self::finish`].
    pub fn render_pass<'e>(
        &'e mut self,
        target: &RenderTarget<'_>,
        label: Option<&str>,
    ) -> wgpu::RenderPass<'e> {
        let color_attachments: Vec<Option<wgpu::RenderPassColorAttachment<'_>>> = target
            .color
            .iter()
            .map(|attachment| {
                Some(wgpu::RenderPassColorAttachment {
                    view: attachment.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: attachment.load,
                        store: attachment.store,
                    },
                })
            })
            .collect();
        let depth_stencil_attachment =
            target
                .depth
                .as_ref()
                .map(|attachment| wgpu::RenderPassDepthStencilAttachment {
                    view: attachment.view,
                    depth_ops: Some(wgpu::Operations {
                        load: attachment.load,
                        store: attachment.store,
                    }),
                    stencil_ops: None,
                });
        self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label,
            color_attachments: &color_attachments,
            depth_stencil_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// The borrowing seam: direct mutable access to the underlying encoder
    /// for recording this type does not model — a staged buffer/texture
    /// upload, or a pass shape this crate does not own.
    ///
    /// A caller reaching for this still owes the type-level invariant: no
    /// submit, and no pass left open across a return.
    pub fn encoder_mut(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Ends recording and hands back the finished `wgpu::CommandBuffer` for
    /// the caller to submit — once, alongside anything else it recorded
    /// before or after this buffer's own passes.
    pub fn finish(self) -> wgpu::CommandBuffer {
        self.encoder.finish()
    }
}
