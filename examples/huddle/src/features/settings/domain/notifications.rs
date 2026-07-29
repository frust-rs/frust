//! Notification preferences — the pure preference-selection vocabulary,
//! split into domain vs. presentation:
//! [`NotificationsController`](crate::features::settings::presentation::controllers::NotificationsController)
//! lives in `presentation::controllers`, hosting this enum's selection as
//! an `RwSignal`.

/// How often the app raises a notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NotifFrequency {
    /// Every message in every joined channel.
    #[default]
    All,
    /// Only direct mentions and DMs.
    Mentions,
    /// Never — fully muted.
    None,
}

impl NotifFrequency {
    /// Every choice, in radio-group order.
    pub const ALL: [NotifFrequency; 3] = [
        NotifFrequency::All,
        NotifFrequency::Mentions,
        NotifFrequency::None,
    ];

    /// The radio-row label.
    pub fn label(self) -> &'static str {
        match self {
            NotifFrequency::All => "All messages",
            NotifFrequency::Mentions => "Mentions & DMs only",
            NotifFrequency::None => "Nothing",
        }
    }

    /// This choice's index in [`NotifFrequency::ALL`].
    pub fn index(self) -> usize {
        NotifFrequency::ALL
            .iter()
            .position(|c| *c == self)
            .expect("self is always a member of ALL")
    }

    /// The choice at radio index `idx` (falls back to the default on an
    /// out-of-range index).
    pub fn from_index(idx: usize) -> Self {
        NotifFrequency::ALL.get(idx).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_index_round_trips() {
        for choice in NotifFrequency::ALL {
            assert_eq!(NotifFrequency::from_index(choice.index()), choice);
        }
    }

    #[test]
    fn from_index_out_of_range_is_the_default() {
        assert_eq!(NotifFrequency::from_index(99), NotifFrequency::default());
    }
}
