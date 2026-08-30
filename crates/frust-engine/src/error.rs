use thiserror::Error;

/// Engine render errors — returned on frame paths, never panicked.
///
/// Mirrors [`vello_hybrid::RenderError`] cases plus engine-specific constraints.
/// The engine always returns errors on invalid or oversized resources rather than
/// panicking, per Frust's frame-path invariant (E17).
#[derive(Debug, Clone, Error)]
pub enum EngineError {
    #[error("atlas allocation failed")]
    AtlasError,

    #[error("missing texture binding")]
    MissingTextureBinding,

    #[error("intermediate texture too large")]
    IntermediateTextureTooLarge,

    #[error("intermediate texture limit reached")]
    IntermediateTextureLimitReached,

    #[error("target exceeds u16 ceiling (65,535 pixels)")]
    TargetTooLarge,

    #[error("non-finite transform refused")]
    InvalidTransform,

    /// A command's own geometry — a rectangle's extents, a corner radius, a
    /// path point, a stroke width, a dash length or phase — is non-finite.
    ///
    /// Separate from [`InvalidTransform`](Self::InvalidTransform) because the
    /// two name different halves of a frame's input: a transform maps geometry
    /// onto the device grid, while this is the geometry itself, and a caller
    /// chasing a blank frame needs to know which of the two it recorded wrong.
    /// Both are refused by the same up-front walk, before any lowering runs.
    #[error("non-finite geometry refused")]
    InvalidGeometry,

    #[error("scheduler escalation")]
    SchedulerEscalation,

    #[error("alpha capacity exhausted")]
    AlphaCapacity,

    #[error("encoded-paint capacity exhausted")]
    PaintCapacity,
}
