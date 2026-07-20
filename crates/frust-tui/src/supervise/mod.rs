//! Async session supervision (PLAN D2/D3).
//!
//! The layer between the TEA engine and `frust-drive`'s pipelines: it wraps
//! the drive's cancellable `spawn_streaming` seam, bridges each session's
//! stdout lines and inferred lifecycle-state changes into one tokio mpsc the
//! engine `select!`s on, and owns the kill/cancel path.
//!
//! - [`session`] is the pure value vocabulary — [`SessionId`], [`SessionSpec`]
//!   (project × device × mode), the [`SessionState`] machine, [`SessionEvent`],
//!   and the [`LaunchPlan`] a spec resolves into.
//! - [`supervisor`] is the moving part — [`Supervisor`] owns a supervision
//!   thread per session and the single event channel.
//!
//! Engine/UI wiring (session tabs, the log view, run-config) lands in TUI2-03/
//! TUI2-04; this module is standalone and unit-tested against a scripted
//! `FakeProcessRunner`.

mod session;
mod supervisor;

pub use session::{
    DeviceTarget, LaunchError, LaunchPlan, SessionEvent, SessionEventKind, SessionId, SessionSpec,
    SessionState,
};
pub use supervisor::Supervisor;
