//! `frust-tui`: a mouse-first ratatui terminal UI for driving Frust apps.
//!
//! Phase 1 (this crate's skeleton) ships the plumbing: the TEA [`engine`]
//! (model + messages + pure `update` + the message channel), the [`ui`] render
//! layer (brand [`ui::theme`], per-frame [`ui::mouse`] region registry with
//! hover, layout shell, welcome + workbench screens), and the [`runner`]
//! terminal lifecycle + tokio event loop. Project detection, session
//! supervision, and the create/bootstrap wizards land in later phases.
//!
//! # Layering (PLAN.md D2)
//!
//! `engine::update` is pure and terminal-free (unit-tested without a TTY) —
//! `AppState::new`/`detect` do bounded filesystem I/O by design (project
//! detection), so "pure" scopes to the message-driven state transition, not
//! every `engine` function. `ui` renders `&AppState` and only ever
//! *registers* interaction as mouse regions — it never mutates the model;
//! `runner` owns the terminal and is the single place raw events become
//! `Message`s. This crate depends on `frust-drive` (the shared drive
//! pipelines) and never on the framework render stack
//! (`frust-core`/`vello`/`wgpu`) — same isolation charter as `frust-cli`.

pub mod engine;
mod runner;
pub mod supervise;
pub mod ui;

pub use runner::run;
