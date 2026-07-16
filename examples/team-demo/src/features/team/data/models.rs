//! Wire-format DTOs. The only place that knows the (simulated) backend's row
//! shape — the domain entity never touches it. See `templates/AGENTS.md`
//! data rules.

use crate::features::team::domain::entities::Member;

/// Wire representation of a member, as the fake backend hands it back.
#[derive(Clone, Debug)]
pub struct MemberRow {
    pub id: String,
    pub name: String,
    pub role: String,
    pub email: String,
}

impl From<MemberRow> for Member {
    fn from(row: MemberRow) -> Self {
        Member {
            id: row.id,
            name: row.name,
            role: row.role,
            email: row.email,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_row_converts_to_domain_entity() {
        let row = MemberRow {
            id: "u1".to_string(),
            name: "Ava Chen".to_string(),
            role: "Mobile Engineer".to_string(),
            email: "ava@team.dev".to_string(),
        };
        let member: Member = row.into();
        assert_eq!(member.id, "u1");
        assert_eq!(member.name, "Ava Chen");
    }
}
