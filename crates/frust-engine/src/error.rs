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

    #[error("non-finite transform or geometry refused")]
    InvalidTransform,

    #[error("scheduler escalation")]
    SchedulerEscalation,

    #[error("alpha capacity exhausted")]
    AlphaCapacity,

    #[error("encoded-paint capacity exhausted")]
    PaintCapacity,
}
