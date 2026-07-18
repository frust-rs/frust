//! Controller registration seam (wave-2 domain wiring) — finalized in task 02.
//!
//! This crate-root module is the single documented place recording where the
//! wave-2 feature controllers plug in. It carries NO logic today: the team
//! roster's own view model stays in
//! [`features::team::presentation::controllers`](crate::features::team::presentation)
//! (`TeamController`), unchanged by this task. Its purpose is to keep the
//! clean-architecture spine legible as screens join — every stateful feature
//! goes through the same `UseCase`/`Controller`/`AsyncState` shape as the
//! `team` slice (PLAN.md's architectural spine).
//!
//! Wave-2 domain modules (each owns its own controller + use cases, hosted
//! inside its screen `Component`):
//!
//! | Feature  | Domain module            | Owning task                       |
//! |----------|--------------------------|-----------------------------------|
//! | notes    | [`crate::notes_domain`]    | task 05 (notes-screen)            |
//! | settings | [`crate::settings_domain`] | task 06 (settings-profile-screens)|
//! | profile  | [`crate::profile_domain`]  | task 06 (settings-profile-screens)|
//!
//! Each wave-2 task fills its `*_domain` module and hosts the controller inside
//! its own `screens/<screen>.rs` `Component` — neither touches this file,
//! `lib.rs`, `routes.rs`, `shell.rs`, or `screens/mod.rs` (the task 02
//! placeholder contract — see the crate docs).
