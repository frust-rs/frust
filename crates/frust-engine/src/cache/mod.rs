//! Engine-side caches for GPU resources derived from a scene.
//!
//! Each cache turns a recurring, expensive-to-derive scene input into a bounded,
//! reusable GPU resource, and is maintained at the frame boundary rather than
//! mid-frame so handles handed out during encoding stay valid for that frame.
//! [`retained`] is the one whose input is a rendered body rather than a decoded
//! resource: a `PushRetained` bracket's pixels, kept so a later frame's
//! `DrawRetained` composites them instead of painting the body again.

pub mod gradients;
pub mod images;
pub mod retained;

pub use gradients::{BYTES_PER_TEXEL, CachedRamp, GradientCache, GradientTextureLayout, LutUpload};
pub use images::{
    AtlasBudget, AtlasRegion, ImageResidency, ImageSkip, ImageUpload, MAX_UNSEEN_FRAMES,
    ResidentImage,
};
pub use retained::{
    RetainedBudget, RetainedDecision, RetainedEntry, RetainedPages, RetainedPlan, RetainedRefusal,
};
