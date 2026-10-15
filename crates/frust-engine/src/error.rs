use thiserror::Error;

/// Engine render errors — returned on frame paths, never panicked.
///
/// Mirrors the reference sparse-strips renderer's error cases plus engine-specific constraints.
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

    /// A frame's layer shape is outside what the engine's scheduler serves.
    ///
    /// `reason` names what was found — a layer graph needing a third live
    /// intermediate page at one pass, a chain deeper than the two-page
    /// ping-pong serves, a filter layer, a non-default blend mode — because the
    /// caller's own log is where a frame that produced no pixels has to be
    /// explainable from.
    ///
    /// Returned rather than panicked (E17), but it is not a route to a second
    /// renderer: the engine tier carries none, and which renderer draws a
    /// surface is settled when that surface is configured, not per frame. The
    /// caller decides what becomes of the frame, and skipping it is the only
    /// answer available — `frust-render`'s engine arm releases the acquired
    /// texture unpresented, counts the refusal and logs `reason` rate-limited,
    /// leaving whatever was presented last on the screen.
    #[error("scheduler escalation: {reason}")]
    SchedulerEscalation {
        /// What the scheduler found that it does not serve.
        reason: String,
    },

    /// A [`Command::DrawRetained`](frust_scene::Command::DrawRetained) named a
    /// retained page the engine cannot composite it from: the key is not
    /// resident (evicted, never recorded, or dropped with the store), or it is
    /// resident at a placement the draw cannot be served from — another linear
    /// transform or extent, or under a coverage-mask clip a composite cannot
    /// honour (see [`crate::cache::retained`]).
    ///
    /// The frame is refused before anything is recorded, exactly like every
    /// other variant here, and the retained store is cleared on the way out:
    /// the resident-key ledger the widget tree painted against was wrong, and
    /// the cheapest way back to an agreeing pair is an empty store whose empty
    /// ledger makes the next frame paint every body again. Logged once per
    /// refusal by the caller, like any refused frame.
    #[error("retained page {key} is not resident at the placement a DrawRetained asked for")]
    RetainedKeyMissing {
        /// The raw key the `DrawRetained` carried.
        key: u64,
    },

    /// A frame records more
    /// [`Command::BackdropBlur`](frust_scene::Command::BackdropBlur)s than the
    /// engine serves in one frame
    /// ([`MAX_BACKDROPS_PER_FRAME`](crate::filters::MAX_BACKDROPS_PER_FRAME)).
    ///
    /// Each backdrop costs the frame a cut of its surface pass, a region copy
    /// and a blur's whole pass sequence, so the count is bounded rather than
    /// left to grow with a widget tree; a frame past the bound is refused up
    /// front, before anything is recorded, exactly like a scheduler
    /// escalation — never rendered with some of its backdrops missing.
    #[error(
        "{recorded} backdrop blurs in one frame, more than the {max} the engine serves per frame"
    )]
    TooManyBackdrops {
        /// How many backdrop blurs the frame recorded.
        recorded: usize,
        /// The per-frame bound.
        max: usize,
    },

    #[error("alpha capacity exhausted")]
    AlphaCapacity,

    #[error("encoded-paint capacity exhausted")]
    PaintCapacity,
}
