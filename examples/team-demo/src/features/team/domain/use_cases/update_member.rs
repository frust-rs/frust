//! `UpdateMember` — renames a member, validating input before ever reaching
//! the repository (validation lives in the use case, per
//! `templates/AGENTS.md` domain rules — not in the controller, not in the
//! view).

use crate::failure::TeamFailure;
use crate::features::team::domain::entities::Member;
use crate::features::team::domain::repositories::TeamRepository;
use clean_signals::UseCase;
use std::sync::Arc;

/// Multi-value params as a small named struct, not a tuple (per
/// `docs/CODE_STANDARDS.md` naming conventions).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateMemberParams {
    pub id: String,
    pub name: String,
}

/// Renames a member. Rejects empty/too-short names with
/// [`TeamFailure::Validation`] before ever calling the repository.
pub struct UpdateMember {
    repo: Arc<dyn TeamRepository + Send + Sync>,
}

impl UpdateMember {
    pub fn new(repo: Arc<dyn TeamRepository + Send + Sync>) -> Self {
        Self { repo }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for UpdateMember {
    type Params = UpdateMemberParams;
    type Output = Member;
    type Failure = TeamFailure;

    async fn execute(&self, params: UpdateMemberParams) -> Result<Member, TeamFailure> {
        let name = params.name.trim();
        if name.is_empty() {
            return Err(TeamFailure::Validation("Name cannot be empty.".to_string()));
        }
        if name.chars().count() < 2 {
            return Err(TeamFailure::Validation(
                "Name must be at least 2 characters.".to_string(),
            ));
        }
        self.repo
            .update_member_name(params.id, name.to_string())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeRepo;

    #[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
    #[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
    impl TeamRepository for FakeRepo {
        async fn list_members(&self) -> Result<Vec<Member>, TeamFailure> {
            unimplemented!("not exercised by UpdateMember tests")
        }

        async fn update_member_name(
            &self,
            id: String,
            name: String,
        ) -> Result<Member, TeamFailure> {
            Ok(Member {
                id,
                name,
                role: "Mobile Engineer".to_string(),
                email: "ava@team.dev".to_string(),
            })
        }
    }

    fn uc() -> UpdateMember {
        let repo: Arc<dyn TeamRepository + Send + Sync> = Arc::new(FakeRepo);
        UpdateMember::new(repo)
    }

    #[tokio::test]
    async fn happy_path_trims_and_updates_name() {
        let result = uc()
            .execute(UpdateMemberParams {
                id: "u1".to_string(),
                name: "  Ava Chen  ".to_string(),
            })
            .await;
        assert_eq!(result.unwrap().name, "Ava Chen");
    }

    #[tokio::test]
    async fn empty_name_is_a_validation_failure() {
        let result = uc()
            .execute(UpdateMemberParams {
                id: "u1".to_string(),
                name: "   ".to_string(),
            })
            .await;
        assert_eq!(
            result,
            Err(TeamFailure::Validation("Name cannot be empty.".to_string()))
        );
    }

    #[tokio::test]
    async fn too_short_name_is_a_validation_failure() {
        let result = uc()
            .execute(UpdateMemberParams {
                id: "u1".to_string(),
                name: "A".to_string(),
            })
            .await;
        assert_eq!(
            result,
            Err(TeamFailure::Validation(
                "Name must be at least 2 characters.".to_string()
            ))
        );
    }
}
