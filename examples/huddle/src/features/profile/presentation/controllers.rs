//! [`ProfileController`] — a **synchronous** lookup against the shared
//! dataset (huddle clean-architecture refactor, task 05; moved verbatim apart
//! from repo threading from the former flat `features::profile` module), no
//! `clean-signals` controller/use-case spine needed: there is no async work
//! here and nothing that can fail, so [`ProfileController::load`] is a plain
//! function taking the injected [`ProfileRepository`], not a
//! `clean_signals_frust::use_controller`-hosted `ControllerCore` (contrast
//! [`crate::features::settings::SettingsController`], which genuinely needs
//! one because `SetTheme` is a real, if infallible, use case run through the
//! spine).
//!
//! [`User`] carries only `id`/`name`/`initials`/`status` — no handle, local
//! time, or role/team fields (the shared dataset is a frozen hub file) — so
//! this module derives those display-only fields deterministically from a
//! user's id, documented as mock flavor text below, not part of the shared
//! dataset's contract.

use crate::features::profile::domain::User;
use crate::features::profile::domain::repositories::ProfileRepository;

/// Rotating role/team flavor strings (mock — deterministic per user id, not
/// part of the shared dataset, so re-rendering the same profile always shows
/// the same values).
const ROLES: [&str; 5] = ["Engineer", "Designer", "Product", "Manager", "Support"];
const TEAMS: [&str; 5] = ["Platform", "Growth", "Core", "Infra", "Design Systems"];

/// The resolved fields
/// [`crate::features::profile::presentation::pages::profile`] renders — a
/// synchronous lookup against the injected [`ProfileRepository`]; `None`
/// from [`ProfileController::load`] means an unknown id (an invalid
/// `/user/:id` deep link/param).
pub struct ProfileController {
    /// The looked-up user.
    pub user: User,
    /// A mock `@handle` derived from the user's name.
    pub handle: String,
    /// A mock "local time" string (no real timezone data anywhere in this
    /// app — deterministic per user id purely for display variety).
    pub local_time: String,
    /// A mock "role · team" line.
    pub role_team: String,
    /// The existing 1:1 DM channel id with this user, if any (drives the
    /// Message action — see
    /// [`crate::features::profile::presentation::pages::profile`]).
    pub dm_channel_id: Option<String>,
}

impl ProfileController {
    /// Resolve `user_id` against `repo`. `None` for an unknown id.
    pub fn load(repo: &(dyn ProfileRepository + Send + Sync), user_id: u32) -> Option<Self> {
        let user = repo.user(user_id)?;
        let handle = format!("@{}", user.name.to_lowercase().replace(' ', "."));
        let role = ROLES[(user_id as usize) % ROLES.len()];
        let team = TEAMS[(user_id as usize) % TEAMS.len()];
        let local_time = mock_local_time(user_id);
        let dm_channel_id = repo
            .dms()
            .into_iter()
            .find(|dm| dm.user_id == user_id)
            .map(|dm| dm.id.to_string());

        Some(Self {
            user,
            handle,
            local_time,
            role_team: format!("{role} · {team}"),
            dm_channel_id,
        })
    }
}

/// A deterministic mock "local time" string for `user_id` (no real timezone
/// support anywhere in this app — flavor text only, see the module docs).
fn mock_local_time(user_id: u32) -> String {
    let hour24 = (user_id * 7) % 24;
    let (hour12, period) = match hour24 {
        0 => (12, "AM"),
        1..=11 => (hour24, "AM"),
        12 => (12, "PM"),
        _ => (hour24 - 12, "PM"),
    };
    format!("{hour12}:00 {period} local time")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::profile::data::repositories::StoreProfileRepository;

    /// A composition-root-equivalent mini-root (task 02's precedent): these
    /// tests assert against the real dataset shape (Grace Hopper/id 2,
    /// `dm-2`), so a fake repository would force assertion rewrites — the
    /// HARD behavior-preserving bar forbids that.
    fn store_repo() -> StoreProfileRepository {
        StoreProfileRepository::new()
    }

    #[test]
    fn unknown_user_id_resolves_to_none() {
        let repo = store_repo();
        assert!(ProfileController::load(&repo, 999).is_none());
    }

    #[test]
    fn known_user_resolves_fields() {
        let repo = store_repo();
        let profile = ProfileController::load(&repo, 2).expect("user 2 exists in the mock dataset");
        assert_eq!(profile.user.name, "Grace Hopper");
        assert_eq!(profile.handle, "@grace.hopper");
        assert_eq!(
            profile.dm_channel_id.as_deref(),
            Some("dm-2"),
            "user 2 has a DM in the mock dataset"
        );
    }

    #[test]
    fn a_user_with_no_dm_resolves_to_none_channel() {
        // The current user (id 1) has no DM entry with themselves.
        let repo = store_repo();
        let profile = ProfileController::load(&repo, 1).expect("user 1 exists");
        assert_eq!(profile.dm_channel_id, None);
    }

    #[test]
    fn local_time_is_deterministic() {
        assert_eq!(mock_local_time(1), mock_local_time(1));
        assert_ne!(mock_local_time(1), mock_local_time(2));
    }
}
