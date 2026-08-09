//! Async session supervision.
//!
//! The layer between the TEA engine and `frust-drive`'s pipelines: it wraps
//! the drive's cancellable `spawn_streaming` seam, bridges each session's
//! stdout lines and inferred lifecycle-state changes into one tokio mpsc the
//! engine `select!`s on, and owns the kill/cancel path.
//!
//! - [`session`] is the pure value vocabulary — [`SessionId`], [`SessionSpec`]
//!   (project × device × mode), the [`SessionState`] machine, [`SessionEvent`],
//!   and the [`LaunchPlan`] a spec resolves into.
//! - [`progress`] is a second pure vocabulary layer — [`PhaseLabel`] and its
//!   extraction from streamed output (`phase_from_output_line`), the
//!   transient build/install/launch status line's source of truth.
//! - [`supervisor`] is the moving part — [`Supervisor`] owns a supervision
//!   thread per session and the single event channel.
//!
//! The desktop `cargo run` path and the multi-phase device pipeline (build →
//! install → launch → logcat, via `frust-drive`'s `android_run`/`ios_run`
//! cancellable seams) both feed the same channel; [`Supervisor::start`]
//! dispatches on the [`SessionSpec`]'s target. The module is unit-tested
//! against a scripted `FakeProcessRunner`.

mod progress;
mod session;
mod supervisor;

pub use progress::{PhaseLabel, phase_from_output_line};
pub use session::{
    DevicePlan, DeviceTarget, LaunchError, LaunchPlan, SessionEvent, SessionEventKind, SessionId,
    SessionSpec, SessionState,
};
pub use supervisor::Supervisor;
