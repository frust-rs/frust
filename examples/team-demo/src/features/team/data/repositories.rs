//! [`InMemoryTeamRepo`] — the [`TeamRepository`] implementation this demo
//! wires up. Owns the translation boundary: transport errors become
//! [`TeamFailure`]s here, DTOs become entities — nothing above this layer
//! knows [`TransportError`] or [`MemberRow`] exist (see `templates/AGENTS.md`
//! data rules).

use crate::failure::TeamFailure;
use crate::features::team::data::sources::{InMemoryTeamSource, TransportError};
use crate::features::team::domain::entities::Member;
use crate::features::team::domain::repositories::TeamRepository;
use std::time::Duration;

pub struct InMemoryTeamRepo {
    source: InMemoryTeamSource,
}

impl InMemoryTeamRepo {
    /// `latency` and `failures_before_success` are forwarded to the
    /// underlying [`InMemoryTeamSource`] — see its docs.
    pub fn new(latency: Duration, failures_before_success: u32) -> Self {
        Self {
            source: InMemoryTeamSource::new(latency, failures_before_success),
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl TeamRepository for InMemoryTeamRepo {
    async fn list_members(&self) -> Result<Vec<Member>, TeamFailure> {
        let rows = self
            .source
            .fetch_members()
            .await
            .map_err(map_transport_error)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn update_member_name(&self, id: String, name: String) -> Result<Member, TeamFailure> {
        let row = self
            .source
            .patch_member_name(&id, &name)
            .await
            .map_err(map_transport_error)?;
        Ok(row.into())
    }
}

/// The single conversion site from the (simulated) transport error to the
/// app's failure type — per `docs/CODE_STANDARDS.md`, no raw transport error
/// may cross the repository boundary.
fn map_transport_error(err: TransportError) -> TeamFailure {
    match err.status {
        404 => TeamFailure::Validation(err.body),
        _ => TeamFailure::Network(err.body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_members_absorbs_configured_failures_then_returns_data() {
        let repo = InMemoryTeamRepo::new(Duration::from_millis(0), 2);

        assert!(repo.list_members().await.is_err());
        assert!(repo.list_members().await.is_err());
        let members = repo.list_members().await.unwrap();
        assert_eq!(members.len(), 6);
    }

    #[tokio::test]
    async fn list_members_maps_transport_failure_to_network_failure() {
        let repo = InMemoryTeamRepo::new(Duration::from_millis(0), 1);
        let err = repo.list_members().await.unwrap_err();
        assert_eq!(
            err,
            TeamFailure::Network("temporary upstream error".to_string())
        );
    }

    #[tokio::test]
    async fn update_member_name_returns_updated_entity() {
        let repo = InMemoryTeamRepo::new(Duration::from_millis(0), 0);
        let member = repo
            .update_member_name("u1".to_string(), "New Name".to_string())
            .await
            .unwrap();
        assert_eq!(member.name, "New Name");
    }

    #[tokio::test]
    async fn update_member_name_maps_missing_id_to_validation_failure() {
        let repo = InMemoryTeamRepo::new(Duration::from_millis(0), 0);
        let err = repo
            .update_member_name("nope".to_string(), "New Name".to_string())
            .await
            .unwrap_err();
        assert!(matches!(err, TeamFailure::Validation(_)));
    }
}
