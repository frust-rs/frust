//! Domain contract for team member data. Implementations live in `data/` and
//! return domain types + `Result<_, TeamFailure>` — never a raw transport
//! error, never a DTO (see `templates/AGENTS.md` domain rules).

use crate::failure::TeamFailure;
use crate::features::team::domain::entities::Member;

/// Same dual `cfg_attr` every async trait in this codebase uses (per
/// `docs/CODE_STANDARDS.md`) — `wasm32`'s single-threaded event loop can't
/// require `Send` futures.
#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
pub trait TeamRepository {
    /// Fetches every team member.
    async fn list_members(&self) -> Result<Vec<Member>, TeamFailure>;

    /// Updates `id`'s display name, returning the updated member.
    async fn update_member_name(&self, id: String, name: String) -> Result<Member, TeamFailure>;
}
