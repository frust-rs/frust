//! Message entities — moved verbatim from the former `mock` module. The
//! shared dataset that materializes these types now lives in
//! [`crate::data::store`].

/// The body of a [`Message`] — text, a link preview stub, or a file stub.
#[derive(Clone, Copy, Debug)]
pub enum MessageBody {
    /// Plain text (may include an `@mention`).
    Text(&'static str),
    /// A link with a preview title (rendered as a `Card` link preview).
    Link {
        /// The URL.
        url: &'static str,
        /// A human title for the preview card.
        title: &'static str,
    },
    /// A file attachment stub (no real bytes — real file attachments are out
    /// of scope for this dataset).
    File {
        /// The file name.
        name: &'static str,
        /// A human size string (e.g. `"2.4 MB"`).
        size: &'static str,
    },
}

/// A single emoji reaction with a count.
#[derive(Clone, Copy, Debug)]
pub struct Reaction {
    /// The emoji (real color-glyph rendering).
    pub emoji: &'static str,
    /// How many members reacted.
    pub count: u32,
}

/// A message in a channel or DM.
#[derive(Clone, Copy, Debug)]
pub struct Message {
    /// Stable numeric id.
    pub id: u32,
    /// The channel or DM this message belongs to.
    pub channel_id: &'static str,
    /// The author.
    pub author_id: u32,
    /// The message body.
    pub body: MessageBody,
    /// Reactions attached to the message.
    pub reactions: &'static [Reaction],
}
