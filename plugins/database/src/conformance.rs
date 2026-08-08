//! Cross-engine conformance suite.
//!
//! **Empty stub.** Wired into `lib.rs` now (`#[cfg(any(test, feature =
//! "conformance"))] pub(crate) mod conformance;`) so no later task needs to
//! touch `lib.rs`'s module list again; the sqlite-backend and
//! turso-backend tasks fill this in with the shared assertions every
//! `engine::EngineConn` backend must satisfy, mirroring
//! `frust-shared-preferences`/`frust-secure-storage`'s own
//! `run_conformance_suite` precedent.
