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
//!
//! # The two caller rules
//!
//! Sharing one encoder between two renderers — a 3D pass of the caller's own
//! and the 2D strip renderer above this crate — turns on two rules the
//! borrowing invariant below cannot express by itself. Both are stated here
//! because both are things a caller otherwise gets wrong silently.
//!
//! ## 1. Depth-clear ownership
//!
//! When both renderers attach the same depth buffer, **exactly one of them
//! clears it, and it is whichever records first**; every pass after that loads
//! what the previous one stored. Clearing twice throws the first pass's
//! occlusion away, and does it with no diagnostic at all — the image simply
//! comes back with the wrong half of it drawn. The 2D renderer's half of the
//! statement is its `set_depth_pre_cleared` switch, which turns its own
//! frame-opening depth clear into a load.
//!
//! Two facts ride along with the clear, because an attachment is only really
//! shared if both sides read it the same way. The **comparison and the
//! direction** must agree: the 2D renderer tests `LessEqual` against a
//! `Depth24Plus` buffer whose far plane is `1.0`, so nearer geometry carries
//! the smaller z, and a pass that inverted either would be occluded exactly
//! where it should not be. And the depth attachment's **extent must equal the
//! colour attachment's** — wgpu refuses the pass outright otherwise, which is
//! at least a loud failure, but sizing the shared buffer against the target is
//! still the caller's job.
//!
//! Colour is not shared on those terms: the 2D renderer clears its colour
//! target every frame, so a caller's earlier pass keeps its depth and loses its
//! pixels. Content that has to stay visible is recorded *after* the frame
//! rather than before it.
//!
//! ## 2. Atlas uploads may submit their own encoder before the scene pass
//!
//! "Never submits" holds for scene work, and glyph-atlas upload is the one
//! carve-out: the atlas is replayed on an encoder of *its own*, submitted ahead
//! of the scene pass, so its content is committed before the pass that samples
//! it reads it (vello_hybrid render/wgpu/mod.rs:428-430). That replay touches
//! neither this buffer nor the caller's and records no scene draw — which is
//! why a caller must not read "the renderer issued a submit" as a contract
//! violation on its own. Any submit that is not this one is.

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
///
/// A caller recording depth-writing passes of its own around a renderer's owes
/// the module-level *two caller rules* on top of this: depth-clear ownership,
/// with the comparison, direction and extent that ride along with it.
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
