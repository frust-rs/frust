//! Notification preferences — data-only settings state (Huddle showcase, task
//! 15).
//!
//! [`NotificationsController`] owns the notification-frequency selection plus a
//! handful of per-type toggle booleans as [`RwSignal`]s, hosted by the
//! notifications screen's `Component` via `clean_signals_frust::use_controller`
//! (the same seam [`super::SettingsController`] uses). Unlike the theming
//! controller, nothing here is applied to the running app — these are pure
//! preference values the screen's [`radio`](frust::radio) group and
//! [`Switch`](frust::Switch)es read and write. It embeds a
//! [`ControllerCore`] by composition purely to satisfy the `use_controller`
//! `AsRef<ControllerCore>` bound and keep the clean-architecture spine visible;
//! it runs no use case.

use clean_signals::ControllerCore;
use frust::RwSignal;

use crate::failure::HuddleFailure;

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

/// View model for the notification-settings screen. See the [module docs](self).
pub struct NotificationsController {
    core: ControllerCore<HuddleFailure>,
    /// The selected notification frequency (drives the radio group).
    pub frequency: RwSignal<NotifFrequency>,
    /// Play a sound on a new notification.
    pub sound: RwSignal<bool>,
    /// Vibrate on a new notification.
    pub vibrate: RwSignal<bool>,
    /// Show a preview of the message body in the notification.
    pub previews: RwSignal<bool>,
}

impl NotificationsController {
    /// A controller seeded with sensible defaults (all-messages frequency,
    /// sound + previews on, vibrate off).
    pub fn new() -> Self {
        Self {
            core: ControllerCore::new(),
            frequency: RwSignal::new(NotifFrequency::default()),
            sound: RwSignal::new(true),
            vibrate: RwSignal::new(false),
            previews: RwSignal::new(true),
        }
    }
}

impl Default for NotificationsController {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for NotificationsController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
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
