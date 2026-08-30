//! The depth attachment the opaque strip pass establishes and the alpha pass
//! tests against.
//!
//! The engine's early-z scheme needs one [`DEPTH_FORMAT`] attachment matching
//! the frame's target extent. Two things own such an attachment, and which one
//! does is the caller's call, not the engine's:
//!
//! 1. **The caller.** A host that already ran a 3D pass into the same target
//!    has a depth buffer with meaningful contents, and the 2D pass has to test
//!    against it rather than against one of its own. That caller passes it in
//!    as `EngineTarget::depth`, and — because it has usually just cleared the
//!    buffer for its own pass — tells the engine so through
//!    [`DepthAttachment::set_pre_cleared`], which turns the frame's depth clear
//!    into a load. Clearing a depth buffer a 3D pass just populated would throw
//!    that pass's occlusion away.
//! 2. **The engine.** With no caller-supplied attachment the engine allocates
//!    one lazily on the first frame that needs it and keeps it across frames,
//!    reallocating only when the extent changes ([`DepthAttachment::resize`]).
//!
//! The texture itself is plain `RENDER_ATTACHMENT`, single mip, single sample:
//! nothing ever samples or copies it, so it needs no other usage, and a
//! downlevel target could not offer one anyway.

use super::pipelines::DEPTH_FORMAT;

/// The depth value a frame clears its attachment to.
///
/// `strip.wgsl` maps the backmost draw to `z = 1.0` and every draw in front of
/// it to a smaller z, so the far plane is the only clear value under which the
/// backmost draw still passes a `LessEqual` test.
pub const DEPTH_CLEAR: f32 = 1.0;

/// The usage every depth attachment is created with.
///
/// Attachment only: the depth buffer is never sampled, never copied out, and
/// never read back, so nothing else belongs here.
pub const DEPTH_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT;

/// The descriptor for a depth attachment covering a `width` x `height` target.
///
/// Both extents are floored at 1: a zero-extent texture cannot be created, and
/// a frame that asked for one has no pixels to depth-test anyway.
#[must_use]
pub fn depth_texture_descriptor(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("frust-engine depth texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: DEPTH_USAGE,
        view_formats: &[],
    }
}

/// The load operation a frame's first depth-using pass applies.
///
/// `pre_cleared` is the caller's statement that the attachment it supplied
/// already holds the depth it wants tested against.
#[must_use]
pub fn depth_load_op(pre_cleared: bool) -> wgpu::LoadOp<f32> {
    if pre_cleared {
        wgpu::LoadOp::Load
    } else {
        wgpu::LoadOp::Clear(DEPTH_CLEAR)
    }
}

/// A depth texture the engine allocated and keeps across frames.
#[derive(Debug)]
pub struct DepthTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl DepthTexture {
    /// Allocates a depth attachment covering a `width` x `height` target.
    #[must_use]
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let descriptor = depth_texture_descriptor(width, height);
        let texture = device.create_texture(&descriptor);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width: descriptor.size.width,
            height: descriptor.size.height,
        }
    }

    /// The underlying texture.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// The full-extent view a render pass attaches to.
    #[must_use]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The `(width, height)` the texture was created at — the *floored*
    /// extent, so a target of zero width answers 1 here.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether this texture is the attachment a `width` x `height` target
    /// needs.
    ///
    /// Exact, not "at least": a depth attachment must match its color
    /// attachment's extent, so a larger leftover from a previous size is no
    /// more usable than a smaller one.
    #[must_use]
    pub fn matches(&self, width: u32, height: u32) -> bool {
        (self.width, self.height) == (width.max(1), height.max(1))
    }
}

/// The engine's depth attachment across frames: an owned texture when the
/// caller supplies none, plus the caller's pre-cleared statement.
#[derive(Debug, Default)]
pub struct DepthAttachment {
    owned: Option<DepthTexture>,
    pre_cleared: bool,
}

impl DepthAttachment {
    /// An attachment owning nothing yet, clearing its depth every frame.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records whether the attachment a caller supplies already holds the
    /// depth it wants tested against, in which case the frame loads it instead
    /// of clearing it.
    ///
    /// Sticky across frames: a host compositing 2D over 3D does so every
    /// frame, so it states this once rather than per frame.
    pub fn set_pre_cleared(&mut self, pre_cleared: bool) {
        self.pre_cleared = pre_cleared;
    }

    /// Whether a caller-supplied attachment is treated as pre-cleared.
    #[must_use]
    pub fn is_pre_cleared(&self) -> bool {
        self.pre_cleared
    }

    /// The load operation this frame's depth clear resolves to.
    ///
    /// Only a caller-supplied attachment can be pre-cleared — an engine-owned
    /// one holds nothing but the previous frame's depth, which is never what
    /// this frame wants to test against.
    #[must_use]
    pub fn load_op(&self, caller_supplied: bool) -> wgpu::LoadOp<f32> {
        depth_load_op(caller_supplied && self.pre_cleared)
    }

    /// The engine-owned attachment, if one has been allocated.
    #[must_use]
    pub fn owned(&self) -> Option<&DepthTexture> {
        self.owned.as_ref()
    }

    /// The engine-owned attachment's view, if one has been allocated.
    #[must_use]
    pub fn owned_view(&self) -> Option<&wgpu::TextureView> {
        self.owned.as_ref().map(DepthTexture::view)
    }

    /// Allocates the engine-owned attachment if there is none at this extent,
    /// and returns its view.
    pub fn ensure(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::TextureView {
        if self
            .owned
            .as_ref()
            .is_some_and(|depth| !depth.matches(width, height))
        {
            self.owned = None;
        }
        self.owned
            .get_or_insert_with(|| DepthTexture::new(device, width, height))
            .view()
    }

    /// Drops the engine-owned attachment, keeping the pre-cleared statement.
    pub fn discard(&mut self) {
        self.owned = None;
    }

    /// Re-establishes the engine-owned attachment at a new extent.
    ///
    /// Only reallocates when one was already owned: a renderer that has never
    /// needed a depth buffer should not start holding one because the surface
    /// changed size. Doing the work here rather than on the next frame keeps
    /// the reallocation off the frame path, where a resize storm would
    /// otherwise pay for it mid-encode.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.owned.is_some() {
            self.owned = Some(DepthTexture::new(device, width, height));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_descriptor_is_a_single_sample_attachment_only_depth_target() {
        let descriptor = depth_texture_descriptor(800, 600);
        assert_eq!(descriptor.format, DEPTH_FORMAT);
        assert_eq!(descriptor.usage, wgpu::TextureUsages::RENDER_ATTACHMENT);
        assert_eq!(descriptor.sample_count, 1);
        assert_eq!(descriptor.mip_level_count, 1);
        assert_eq!(descriptor.size.width, 800);
        assert_eq!(descriptor.size.height, 600);
        assert_eq!(descriptor.size.depth_or_array_layers, 1);
    }

    #[test]
    fn a_zero_extent_is_floored_to_one_rather_than_refused() {
        let descriptor = depth_texture_descriptor(0, 0);
        assert_eq!((descriptor.size.width, descriptor.size.height), (1, 1));
    }

    #[test]
    fn a_pre_cleared_attachment_loads_instead_of_clearing() {
        assert_eq!(depth_load_op(false), wgpu::LoadOp::Clear(DEPTH_CLEAR));
        assert_eq!(depth_load_op(true), wgpu::LoadOp::Load);
    }

    #[test]
    fn only_a_caller_supplied_attachment_can_be_pre_cleared() {
        let mut depth = DepthAttachment::new();
        assert!(!depth.is_pre_cleared());
        assert_eq!(depth.load_op(true), wgpu::LoadOp::Clear(DEPTH_CLEAR));

        depth.set_pre_cleared(true);
        assert!(depth.is_pre_cleared());
        assert_eq!(depth.load_op(true), wgpu::LoadOp::Load);
        // An engine-owned buffer holds only the previous frame's depth, so the
        // statement does not carry over to it.
        assert_eq!(depth.load_op(false), wgpu::LoadOp::Clear(DEPTH_CLEAR));
    }

    #[test]
    fn an_attachment_owns_nothing_until_a_frame_asks_for_one() {
        let depth = DepthAttachment::new();
        assert!(depth.owned().is_none());
        assert!(depth.owned_view().is_none());
    }
}
