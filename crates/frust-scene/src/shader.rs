//! [`ShaderProgram`]: a runtime fragment-shader handle (WGSL source).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-unique id counter backing [`ShaderProgram::new`].
static NEXT_SHADER_PROGRAM_ID: AtomicU64 = AtomicU64::new(1);

/// A runtime fragment-shader program (WGSL source) — the Frust analog of
/// Flutter's `FragmentProgram`.
///
/// This is a renderer contract, not a scene-layer dependency: the WGSL
/// source is carried here as an opaque string (precedent:
/// `frust_theme::GlassMaterial::blur_radius_intent`, a future-backend
/// contract that is likewise plain data rather than a compiled resource) —
/// only `frust-render` compiles it into a GPU pipeline
/// (`workflow/plans/features/frust-shader-showcase/tasks/03-render-shader-effects.md`).
///
/// **v1 contract**: the shader MUST write opaque output (alpha = 1.0) — see
/// `workflow/plans/features/frust-shader-showcase/research/RESEARCH.md` §Q1
/// (vello's image-override copy is bit-for-bit, and Frust's render target is
/// premultiplied; the conventions only coincide at alpha = 1.0).
#[derive(Clone, Debug)]
pub struct ShaderProgram {
    /// Process-unique id, minted fresh by [`ShaderProgram::new`] and shared
    /// across `Clone` — the compile-once cache key `frust-render` uses to
    /// avoid recompiling the same WGSL source every frame.
    id: u64,
    /// WGSL fragment source. `Arc<str>` keeps a clone cheap (a handle copy,
    /// not a string copy) and `Send` (no `Sync` requirement) — the Scene:
    /// Send tripwire (`lib.rs`) this payload must satisfy for the
    /// render-thread split (RESEARCH.md §Q2).
    source: Arc<str>,
}

impl ShaderProgram {
    /// Wraps `wgsl` in a new program handle, minting a fresh process-unique
    /// id.
    ///
    /// `Clone` shares the id — see the `id` field's doc comment.
    ///
    /// # Cache-once contract
    ///
    /// Call this **once** per distinct shader and retain (or `Clone`) the
    /// result — never mint a fresh `ShaderProgram` every frame or rebuild.
    /// `frust-render`'s shader-effects engine compiles and caches a GPU
    /// pipeline keyed by [`id()`](Self::id): a fresh id every frame is a
    /// permanent cache miss, forcing a full pipeline recompile (plus
    /// target/registration churn) every single frame instead of the
    /// intended compile-once-then-reuse cost.
    ///
    /// Create it once in retained state — a `Component`'s `init`, or other
    /// `Widget`/`View` state built once and reused — and clone the handle
    /// (cheap: an `Arc` handle copy, sharing the id) wherever it's drawn
    /// thereafter. A `View`'s own `build` method is a correct create-once
    /// hook too: it constructs the retained widget exactly once, so minting
    /// a program there is fine. Do **not** call `ShaderProgram::new` inline
    /// inside a widget's `paint` method (runs every frame) or inside a
    /// `Component`'s `build` (re-runs every rebuild) — both mint a new id on
    /// every call. See `examples/shadertoy/src/shaders.rs`'s `all()` for the
    /// reference pattern: a registry built once and reused.
    pub fn new(wgsl: impl Into<Arc<str>>) -> Self {
        let id = NEXT_SHADER_PROGRAM_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            source: wgsl.into(),
        }
    }

    /// The process-unique id, stable across `Clone`.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The WGSL fragment source.
    pub fn source(&self) -> &str {
        &self.source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mints_unique_ids() {
        let a = ShaderProgram::new("fn a() {}");
        let b = ShaderProgram::new("fn b() {}");
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn new_mints_unique_ids_across_threads() {
        let handles: Vec<_> = (0..8)
            .map(|i| std::thread::spawn(move || ShaderProgram::new(format!("fn s{i}() {{}}")).id()))
            .collect();
        let mut ids: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 8, "all ids must be unique across threads");
    }

    #[test]
    fn clone_shares_id() {
        let a = ShaderProgram::new("fn a() {}");
        let b = a.clone();
        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn source_round_trips() {
        let program = ShaderProgram::new("fn main() {}");
        assert_eq!(program.source(), "fn main() {}");
    }
}
