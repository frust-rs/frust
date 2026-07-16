//! In-memory stand-in for a remote team API.
//!
//! Simulates what makes real backends annoying: [`InMemoryTeamSource::latency`]
//! on every call (via `clean_signals::time::sleep`, never a raw
//! `tokio`/`gloo` sleep — see `docs/CODE_STANDARDS.md`), and the first
//! `failures_before_success` list fetches failing with a transient transport
//! error — the repository maps that to a retryable
//! [`crate::failure::TeamFailure::Network`], and `TeamController::load`'s
//! `RetryPolicy` absorbs it transparently.

use crate::features::team::data::models::MemberRow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Transport-level error, as an HTTP client would surface it. Never leaves
/// the data layer — [`super::repositories::InMemoryTeamRepo`] maps it to
/// [`crate::failure::TeamFailure`] at exactly one site.
#[derive(Clone, Debug)]
pub struct TransportError {
    pub status: u16,
    pub body: String,
}

pub struct InMemoryTeamSource {
    latency: Duration,
    failures_before_success: u32,
    list_fetches: AtomicU32,
    db: Mutex<Vec<MemberRow>>,
}

impl InMemoryTeamSource {
    /// `latency` is applied to every call; the first `failures_before_success`
    /// calls to [`Self::fetch_members`] fail with a transient
    /// [`TransportError`] before the (seeded) data is ever returned.
    pub fn new(latency: Duration, failures_before_success: u32) -> Self {
        Self {
            latency,
            failures_before_success,
            list_fetches: AtomicU32::new(0),
            db: Mutex::new(seed_rows()),
        }
    }

    pub async fn fetch_members(&self) -> Result<Vec<MemberRow>, TransportError> {
        clean_signals::time::sleep(self.latency).await;
        let attempt = self.list_fetches.fetch_add(1, Ordering::SeqCst);
        if attempt < self.failures_before_success {
            return Err(TransportError {
                status: 500,
                body: "temporary upstream error".to_string(),
            });
        }
        Ok(self.db.lock().unwrap().clone())
    }

    pub async fn patch_member_name(
        &self,
        id: &str,
        name: &str,
    ) -> Result<MemberRow, TransportError> {
        clean_signals::time::sleep(self.latency).await;
        let mut db = self.db.lock().unwrap();
        match db.iter_mut().find(|row| row.id == id) {
            Some(row) => {
                row.name = name.to_string();
                Ok(row.clone())
            }
            None => Err(TransportError {
                status: 404,
                body: format!("no member {id}"),
            }),
        }
    }
}

fn seed_rows() -> Vec<MemberRow> {
    [
        ("u1", "Ava Chen", "Mobile Engineer", "ava@team.dev"),
        ("u2", "Bruno Costa", "Backend Engineer", "bruno@team.dev"),
        ("u3", "Chidi Okafor", "Product Designer", "chidi@team.dev"),
        ("u4", "Dana Weiss", "Engineering Manager", "dana@team.dev"),
        ("u5", "Emre Yilmaz", "QA Engineer", "emre@team.dev"),
        ("u6", "Freja Lund", "Mobile Engineer", "freja@team.dev"),
    ]
    .into_iter()
    .map(|(id, name, role, email)| MemberRow {
        id: id.to_string(),
        name: name.to_string(),
        role: role.to_string(),
        email: email.to_string(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_members_fails_configured_times_then_succeeds() {
        let source = InMemoryTeamSource::new(Duration::from_millis(0), 2);

        assert!(source.fetch_members().await.is_err());
        assert!(source.fetch_members().await.is_err());
        let rows = source.fetch_members().await.unwrap();
        assert_eq!(rows.len(), 6);
    }

    #[tokio::test]
    async fn patch_member_name_updates_matching_row() {
        let source = InMemoryTeamSource::new(Duration::from_millis(0), 0);
        let row = source.patch_member_name("u1", "New Name").await.unwrap();
        assert_eq!(row.name, "New Name");
    }

    #[tokio::test]
    async fn patch_member_name_errors_for_unknown_id() {
        let source = InMemoryTeamSource::new(Duration::from_millis(0), 0);
        let err = source
            .patch_member_name("nope", "New Name")
            .await
            .unwrap_err();
        assert_eq!(err.status, 404);
    }
}
