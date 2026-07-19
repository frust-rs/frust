//! `messages` domain models — the owned, mutable working copies the feed and
//! thread render from, plus the message-load use-case params (huddle
//! clean-architecture refactor, task 03; moved from the former flat
//! `features/messages/mod.rs`).
//!
//! [`FeedMessage`] is the owned counterpart to the immutable
//! [`Message`](super::entities::Message) entity: reactions toggle, a thread
//! grows, and runtime-authored messages (a send, a canned reply) append to a
//! feed the static dataset never mutates. These are plain data + behavior
//! (Design Decision 2 lets a domain model reuse another feature's domain — the
//! current-user id below comes from `profile::domain`); nothing here reaches
//! the data layer or `frust`.

use std::time::Duration;

use crate::features::messages::domain::entities::{Message, MessageBody};
use crate::features::profile::domain::CURRENT_USER_ID;

/// How many messages a page holds — the newest page loaded first, and the size
/// of each older page [`MessagesController::load_older`](crate::features::messages::MessagesController)
/// prepends.
pub const PAGE_SIZE: usize = 30;

/// The body of a [`FeedMessage`] — the owned counterpart to
/// [`MessageBody`] (a sent message carries a runtime `String`, not the
/// `&'static str` the static dataset uses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedBody {
    /// Plain text (may include an `@mention`).
    Text(String),
    /// A link with a preview title (rendered as a `Card` link preview).
    Link {
        /// The URL.
        url: String,
        /// A human title for the preview card.
        title: String,
    },
    /// A file attachment stub (rendered as an `icons::DESCRIPTION` file card).
    File {
        /// The file name.
        name: String,
        /// A human size string (e.g. `"2.4 MB"`).
        size: String,
    },
}

impl FeedBody {
    fn from_mock(body: MessageBody) -> Self {
        match body {
            MessageBody::Text(t) => FeedBody::Text(t.to_string()),
            MessageBody::Link { url, title } => FeedBody::Link {
                url: url.to_string(),
                title: title.to_string(),
            },
            MessageBody::File { name, size } => FeedBody::File {
                name: name.to_string(),
                size: size.to_string(),
            },
        }
    }
}

/// A single emoji reaction with a count and whether the current user is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedReaction {
    /// The emoji character (real color-glyph rendering, verified in wave A).
    pub emoji: String,
    /// How many members reacted.
    pub count: u32,
    /// Whether [`CURRENT_USER_ID`] is one of them (drives the chip's
    /// selected state).
    pub mine: bool,
}

/// One reply inside a message's thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedReply {
    /// Who wrote the reply.
    pub author_id: u32,
    /// The reply text.
    pub text: String,
}

/// A message in the feed — the owned, mutable working copy of a
/// [`Message`] (reactions toggle, a thread grows), plus the runtime
/// messages a send/canned-reply appends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedMessage {
    /// Stable id (mock ids for loaded messages, [`RUNTIME_ID_BASE`](crate::features::messages::MessagesController)+ for sent).
    pub id: u32,
    /// The author.
    pub author_id: u32,
    /// The message body.
    pub body: FeedBody,
    /// Reactions attached to the message.
    pub reactions: Vec<FeedReaction>,
    /// The message's thread (empty until replied to).
    pub replies: Vec<FeedReply>,
}

impl FeedMessage {
    /// The owned working copy of a static [`Message`], seeding a deterministic
    /// subset with a couple of canned thread replies.
    pub fn from_mock(m: Message) -> Self {
        let reactions = m
            .reactions
            .iter()
            .map(|r| FeedReaction {
                emoji: r.emoji.to_string(),
                count: r.count,
                mine: false,
            })
            .collect();
        let mut msg = FeedMessage {
            id: m.id,
            author_id: m.author_id,
            body: FeedBody::from_mock(m.body),
            reactions,
            replies: Vec::new(),
        };
        // Seed a couple of canned replies on a deterministic subset so the feed
        // shows the "N replies →" thread affordance without any thread data in
        // the mock layer.
        if m.id.is_multiple_of(4) {
            msg.replies = vec![
                FeedReply {
                    author_id: 3,
                    text: "Following up in the thread.".to_string(),
                },
                FeedReply {
                    author_id: 5,
                    text: "Same, added a note.".to_string(),
                },
            ];
        }
        msg
    }

    /// A message the current user just typed and sent.
    pub fn own_text(id: u32, text: String) -> Self {
        FeedMessage {
            id,
            author_id: CURRENT_USER_ID,
            body: FeedBody::Text(text),
            reactions: Vec::new(),
            replies: Vec::new(),
        }
    }

    /// The canned reply the mock backend "types" back after a send.
    pub fn canned_reply(id: u32) -> Self {
        FeedMessage {
            id,
            // Grace Hopper stands in for the channel's other members.
            author_id: 2,
            body: FeedBody::Text("Got it — thanks for the update!".to_string()),
            reactions: Vec::new(),
            replies: Vec::new(),
        }
    }

    /// Whether this message was authored by [`CURRENT_USER_ID`] (own
    /// messages render right-aligned).
    pub fn is_own(&self) -> bool {
        self.author_id == CURRENT_USER_ID
    }

    /// The thread reply count shown on the bubble.
    pub fn reply_count(&self) -> usize {
        self.replies.len()
    }

    /// Toggle the current user's reaction with `emoji`: add it (count 1) if
    /// absent, join an existing one (+1), or leave one already joined (-1,
    /// removing the chip at zero).
    pub fn toggle_reaction(&mut self, emoji: &str) {
        if let Some(pos) = self.reactions.iter().position(|r| r.emoji == emoji) {
            let reaction = &mut self.reactions[pos];
            if reaction.mine {
                reaction.count = reaction.count.saturating_sub(1);
                reaction.mine = false;
                if reaction.count == 0 {
                    self.reactions.remove(pos);
                }
            } else {
                reaction.count += 1;
                reaction.mine = true;
            }
        } else {
            self.reactions.push(FeedReaction {
                emoji: emoji.to_string(),
                count: 1,
                mine: true,
            });
        }
    }
}

/// Parameters for [`LoadMessages`](super::use_cases::LoadMessages): the channel
/// to load, an optional newest-page cap (the firehose loads its newest
/// [`PAGE_SIZE`]), and the mock latency to simulate.
#[derive(Clone, Debug)]
pub struct LoadMessagesParams {
    /// The channel or DM id.
    pub channel_id: String,
    /// `Some(n)` loads only the newest `n` (firehose pagination); `None` loads
    /// the whole channel.
    pub limit: Option<usize>,
    /// The simulated network latency.
    pub latency: Duration,
}

#[cfg(test)]
mod tests {
    //! Pure-logic unit tests (no runtime/render). The async load / send /
    //! pagination behaviors and the screen render are exercised by the
    //! integration suite in `tests/feed.rs`.
    use super::*;

    fn text_message(id: u32, author_id: u32) -> FeedMessage {
        FeedMessage {
            id,
            author_id,
            body: FeedBody::Text("hi".to_string()),
            reactions: Vec::new(),
            replies: Vec::new(),
        }
    }

    #[test]
    fn reaction_toggle_adds_joins_and_removes() {
        let mut msg = text_message(1, 2);
        // Add fresh.
        msg.toggle_reaction("\u{1F525}");
        assert_eq!(msg.reactions.len(), 1);
        assert_eq!(msg.reactions[0].count, 1);
        assert!(msg.reactions[0].mine);

        // A second user's reaction the current user then joins.
        msg.reactions[0].mine = false;
        msg.reactions[0].count = 2;
        msg.toggle_reaction("\u{1F525}");
        assert_eq!(msg.reactions[0].count, 3, "joining bumps the count");
        assert!(msg.reactions[0].mine);

        // Toggling off decrements; at zero the chip is removed.
        msg.toggle_reaction("\u{1F525}");
        assert_eq!(msg.reactions[0].count, 2);
        assert!(!msg.reactions[0].mine);
        msg.reactions[0].count = 1;
        msg.reactions[0].mine = true;
        msg.toggle_reaction("\u{1F525}");
        assert!(
            msg.reactions.is_empty(),
            "the last own reaction removes the chip"
        );
    }

    #[test]
    fn own_messages_are_authored_by_the_current_user() {
        assert!(text_message(1, CURRENT_USER_ID).is_own());
        assert!(!text_message(1, 2).is_own());
    }

    #[test]
    fn from_mock_seeds_a_thread_on_the_deterministic_subset() {
        let threaded = FeedMessage::from_mock(Message {
            id: 4,
            channel_id: "general",
            author_id: 2,
            body: MessageBody::Text("x"),
            reactions: &[],
        });
        assert_eq!(threaded.reply_count(), 2, "id % 4 == 0 seeds a thread");

        let plain = FeedMessage::from_mock(Message {
            id: 5,
            channel_id: "general",
            author_id: 2,
            body: MessageBody::Text("x"),
            reactions: &[],
        });
        assert_eq!(plain.reply_count(), 0);
    }
}
