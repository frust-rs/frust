//! `profile` domain — the repository seam over the shared dataset (huddle
//! clean-architecture refactor, task 05).
//!
//! **Synchronous, infallible** (PLAN Design Decision 8): the profile feature
//! has no `ControllerCore`/`HuddleFailure` spine — there is no async work
//! here and nothing that can fail — so this trait mirrors the former
//! `mock`-shim accessors' exact shape rather than the
//! `async fn ... -> Result<_, HuddleFailure>` shape the four fallible
//! features use. Shaped by what
//! [`ProfileController::load`](crate::features::profile::presentation::controllers::ProfileController::load)
//! and `presentation::pages::you`'s current-user lookup actually read: a
//! user lookup by id (the current-user lookup is the same method, called
//! with [`super::entities::CURRENT_USER_ID`]) and the DM roster (to find an
//! existing 1:1 conversation with the looked-up user).

use crate::features::channels::domain::Dm;

use super::entities::User;

/// The profile feature's data seam.
pub trait ProfileRepository {
    /// Look up a member by id. `None` for an unknown id.
    fn user(&self, id: u32) -> Option<User>;
    /// All direct-message conversations — [`ProfileController::load`](crate::features::profile::presentation::controllers::ProfileController::load)
    /// scans these for an existing 1:1 DM with the looked-up user.
    fn dms(&self) -> Vec<Dm>;
}
