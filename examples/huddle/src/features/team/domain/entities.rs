//! Plain domain entities for the `team` feature. No serde, no framework
//! types — see `docs/CODE_STANDARDS.md` / `templates/AGENTS.md` domain rules.

/// A team member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub role: String,
    pub email: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_is_a_plain_cloneable_comparable_struct() {
        let a = Member {
            id: "u1".to_string(),
            name: "Ava Chen".to_string(),
            role: "Mobile Engineer".to_string(),
            email: "ava@team.dev".to_string(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
