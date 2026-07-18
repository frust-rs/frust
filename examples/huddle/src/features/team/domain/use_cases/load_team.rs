//! `LoadTeam` — fetches the full member list. Parameterless (`NoParams`).

use crate::failure::TeamFailure;
use crate::features::team::domain::entities::Member;
use crate::features::team::domain::repositories::TeamRepository;
use clean_signals::{NoParams, UseCase};
use std::sync::Arc;

/// Loads every team member via the injected [`TeamRepository`].
///
/// Takes the repository as `Arc<dyn TeamRepository + Send + Sync>` (a trait
/// object, per the DI conventions in `templates/AGENTS.md`) rather than a
/// generic parameter, so the presentation layer's controller/page types stay
/// free of a repository type parameter.
///
/// Never call [`UseCase::execute`] directly from a controller — always drive
/// this through `ControllerCore::run`/`run_into`, which layers on retry,
/// activity tracking, and failure routing (see `docs/CODE_STANDARDS.md`
/// anti-patterns).
pub struct LoadTeam {
    repo: Arc<dyn TeamRepository + Send + Sync>,
}

impl LoadTeam {
    pub fn new(repo: Arc<dyn TeamRepository + Send + Sync>) -> Self {
        Self { repo }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadTeam {
    type Params = NoParams;
    type Output = Vec<Member>;
    type Failure = TeamFailure;

    async fn execute(&self, _params: NoParams) -> Result<Vec<Member>, TeamFailure> {
        self.repo.list_members().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal in-domain fake — domain tests never reach into `data/`.
    struct FakeRepo {
        members: Vec<Member>,
        fail: bool,
    }

    #[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
    #[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
    impl TeamRepository for FakeRepo {
        async fn list_members(&self) -> Result<Vec<Member>, TeamFailure> {
            if self.fail {
                Err(TeamFailure::Network("boom".to_string()))
            } else {
                Ok(self.members.clone())
            }
        }

        async fn update_member_name(
            &self,
            _id: String,
            _name: String,
        ) -> Result<Member, TeamFailure> {
            unimplemented!("not exercised by LoadTeam tests")
        }
    }

    fn member(id: &str) -> Member {
        Member {
            id: id.to_string(),
            name: "Ava Chen".to_string(),
            role: "Mobile Engineer".to_string(),
            email: "ava@team.dev".to_string(),
        }
    }

    #[tokio::test]
    async fn happy_path_returns_repository_members() {
        let repo: Arc<dyn TeamRepository + Send + Sync> = Arc::new(FakeRepo {
            members: vec![member("u1"), member("u2")],
            fail: false,
        });
        let uc = LoadTeam::new(repo);

        let result = uc.execute(NoParams).await;
        assert_eq!(result, Ok(vec![member("u1"), member("u2")]));
    }

    #[tokio::test]
    async fn failure_path_propagates_repository_failure() {
        let repo: Arc<dyn TeamRepository + Send + Sync> = Arc::new(FakeRepo {
            members: vec![],
            fail: true,
        });
        let uc = LoadTeam::new(repo);

        let result = uc.execute(NoParams).await;
        assert_eq!(result, Err(TeamFailure::Network("boom".to_string())));
    }
}
