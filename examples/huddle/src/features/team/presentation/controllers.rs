//! [`TeamController`] — the team list screen's view model. Embeds a
//! [`ControllerCore`] by composition (see `templates/AGENTS.md` controller
//! rules) and owns every signal the screen reads.

use crate::failure::TeamFailure;
use crate::features::team::domain::entities::Member;
use crate::features::team::domain::repositories::TeamRepository;
use crate::features::team::domain::use_cases::{LoadTeam, UpdateMember, UpdateMemberParams};
use clean_signals::{
    AsyncState, ControllerCore, FailureSink, NoParams, RetryPolicy, RunOptions, async_state_signal,
};
// `reactive_graph` isn't a direct dependency of this crate — its signal types
// and traits are reached through the ForgeKit facade's flat re-exports
// (`forgekit::{Memo, RwSignal, Get, Update}`), exactly as
// `docs/CODE_STANDARDS.md`'s "State & Reactivity Conventions" prescribe: an app
// crate names `forgekit` only, never `reactive_graph`/`any_spawner` directly.
// (The Leptos twin reached the identical `reactive_graph` types through
// `leptos::prelude`; ForgeKit re-exports the same crate, so the signal types
// unify with clean-signals' — the type-identity rule.)
use forgekit::{Get, Memo, RwSignal, Update};
use std::sync::Arc;
use std::time::Duration;

/// View model for the team list screen.
///
/// Fine-grained state: [`Self::members`] holds the async list, [`Self::query`]
/// the search text, and [`Self::filtered`] is derived from both.
pub struct TeamController {
    core: ControllerCore<TeamFailure>,
    load_team: LoadTeam,
    update_member: UpdateMember,
    /// The member list through its load lifecycle (loading / data / error).
    pub members: RwSignal<AsyncState<Vec<Member>, TeamFailure>>,
    /// Search box text (name or role).
    pub query: RwSignal<String>,
    /// Members matching [`Self::query`]; empty while [`Self::members`] has no
    /// value yet.
    pub filtered: Memo<Vec<Member>>,
}

impl TeamController {
    pub fn new(repo: Arc<dyn TeamRepository + Send + Sync>) -> Self {
        let members = async_state_signal::<Vec<Member>, TeamFailure>();
        let query = RwSignal::new(String::new());
        let filtered = Memo::new(move |_: Option<&Vec<Member>>| {
            let Some(all) = members.get().value().cloned() else {
                return Vec::new();
            };
            let needle = query.get().trim().to_lowercase();
            if needle.is_empty() {
                return all;
            }
            all.into_iter()
                .filter(|m| {
                    m.name.to_lowercase().contains(&needle)
                        || m.role.to_lowercase().contains(&needle)
                })
                .collect()
        });

        Self {
            core: ControllerCore::new(),
            load_team: LoadTeam::new(Arc::clone(&repo)),
            update_member: UpdateMember::new(repo),
            members,
            query,
            filtered,
        }
    }

    /// Loads the member list. The composition root's fake backend fails the
    /// first call with a retryable `TeamFailure::Network`; this policy absorbs
    /// it transparently.
    pub async fn load(&self) {
        let _ = self
            .core
            .run_into(
                &self.load_team,
                NoParams,
                self.members,
                RunOptions {
                    retry: RetryPolicy::new(3, Duration::from_millis(200)).with_backoff(2.0),
                    ..Default::default()
                },
            )
            .await;
    }

    /// Renames a member. Validation failures (empty/too-short name) are
    /// non-retryable and surface on [`Self::failures`] like any other
    /// failure — the caller never needs to inspect the `Result` for error
    /// handling, only to react to success.
    pub async fn rename(&self, id: String, name: String) {
        let result = self
            .core
            .run(
                &self.update_member,
                UpdateMemberParams { id, name },
                RunOptions::default(),
            )
            .await;

        // The failure path was already routed to the failure sink by
        // `core.run` above; here we only need to react to success by
        // patching the already-loaded list in place.
        if let Ok(updated) = result {
            self.members.try_update(|state| {
                if let AsyncState::Data(all) | AsyncState::Reloading(all) = state
                    && let Some(existing) = all.iter_mut().find(|m| m.id == updated.id)
                {
                    *existing = updated;
                }
            });
        }
    }

    /// The controller's failure event sink. The screen subscribes once, near
    /// the feature's UI root, to surface failures (see
    /// `clean_signals_forgekit::use_failure_listener`).
    pub fn failures(&self) -> &FailureSink<TeamFailure> {
        self.core.failures()
    }
}

impl AsRef<ControllerCore<TeamFailure>> for TeamController {
    fn as_ref(&self) -> &ControllerCore<TeamFailure> {
        &self.core
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clean_signals::Failure;
    use forgekit::{GetUntracked, Set};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A fake repository whose `list_members` fails a configurable number of
    /// times before succeeding — proves the controller's retry policy
    /// absorbs exactly that many failures.
    struct FlakyRepo {
        fail_times: u32,
        attempts: AtomicU32,
        members: Vec<Member>,
    }

    fn member(id: &str, name: &str) -> Member {
        Member {
            id: id.to_string(),
            name: name.to_string(),
            role: "Mobile Engineer".to_string(),
            email: format!("{id}@team.dev"),
        }
    }

    impl FlakyRepo {
        fn new(fail_times: u32) -> Self {
            Self {
                fail_times,
                attempts: AtomicU32::new(0),
                members: vec![
                    member("u1", "Ava Chen"),
                    member("u2", "Bruno Costa"),
                    member("u3", "Chidi Okafor"),
                ],
            }
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
    #[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
    impl TeamRepository for FlakyRepo {
        async fn list_members(&self) -> Result<Vec<Member>, TeamFailure> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if attempt <= self.fail_times {
                Err(TeamFailure::Network("temporary upstream error".to_string()))
            } else {
                Ok(self.members.clone())
            }
        }

        async fn update_member_name(
            &self,
            id: String,
            name: String,
        ) -> Result<Member, TeamFailure> {
            Ok(member(&id, &name))
        }
    }

    #[tokio::test]
    async fn load_absorbs_two_flaky_failures_then_succeeds() {
        let repo = Arc::new(FlakyRepo::new(2));
        let controller =
            TeamController::new(Arc::clone(&repo) as Arc<dyn TeamRepository + Send + Sync>);

        controller.load().await;

        assert_eq!(
            repo.attempts.load(Ordering::SeqCst),
            3,
            "retry absorbed exactly 2 failures"
        );
        assert!(controller.members.get_untracked().has_value());
        controller.core.dispose();
    }

    #[tokio::test]
    async fn filtered_memo_narrows_on_query_change() {
        let repo: Arc<dyn TeamRepository + Send + Sync> = Arc::new(FlakyRepo::new(0));
        let controller = TeamController::new(repo);
        controller.load().await;

        assert_eq!(
            controller.filtered.get_untracked().len(),
            3,
            "unfiltered shows all"
        );

        controller.query.set("bruno".to_string());
        let filtered = controller.filtered.get_untracked();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "u2");

        controller.query.set("nonexistent".to_string());
        assert!(controller.filtered.get_untracked().is_empty());

        controller.core.dispose();
    }

    #[tokio::test]
    async fn rename_with_empty_name_surfaces_validation_failure_via_sink() {
        let repo: Arc<dyn TeamRepository + Send + Sync> = Arc::new(FlakyRepo::new(0));
        let controller = TeamController::new(repo);

        let failures = Arc::new(Mutex::new(Vec::<TeamFailure>::new()));
        let sink = Arc::clone(&failures);
        controller
            .failures()
            .subscribe(move |f: &TeamFailure| sink.lock().unwrap().push(f.clone()))
            .forget();

        controller.rename("u1".to_string(), "   ".to_string()).await;

        let captured = failures.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0],
            TeamFailure::Validation("Name cannot be empty.".to_string())
        );
        assert!(!captured[0].is_retryable());

        controller.core.dispose();
    }

    #[tokio::test]
    async fn rename_success_patches_the_loaded_list_in_place() {
        let repo = Arc::new(FlakyRepo::new(0));
        let controller =
            TeamController::new(Arc::clone(&repo) as Arc<dyn TeamRepository + Send + Sync>);
        controller.load().await;

        controller
            .rename("u2".to_string(), "New Name".to_string())
            .await;

        let members = controller.members.get_untracked();
        let updated = members
            .value()
            .unwrap()
            .iter()
            .find(|m| m.id == "u2")
            .unwrap();
        assert_eq!(updated.name, "New Name");
        controller.core.dispose();
    }
}
