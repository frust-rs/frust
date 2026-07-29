//! Activity entities — moved verbatim from the former `mock` module (huddle
//! clean-architecture refactor). The shared dataset that
//! materializes these types now lives in [`crate::data::store`].

/// An item in the Activity tab — a mention derived from the message stream.
#[derive(Clone, Copy, Debug)]
pub struct ActivityItem {
    /// The message that produced this activity item.
    pub message_id: u32,
    /// The channel the mention happened in.
    pub channel_id: &'static str,
    /// Who mentioned the current user.
    pub author_id: u32,
    /// The mention text.
    pub text: &'static str,
}
