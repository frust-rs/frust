//! Messages feature domain — the channel/DM message-feed spine.
//!
//! [`MessagesController`] is the view model for one channel or DM. It owns
//! every signal the feed and thread screens render and drives the
//! mock-latency load / send / react / thread-reply / pagination operations
//! through an embedded [`ControllerCore`] (the `templates/AGENTS.md`
//! controller rule, same shape as [`crate::features::settings`]).
//!
//! # One shared controller per channel
//!
//! [`MessagesController::for_channel`] is a per-`channel_id` registry (a
//! `thread_local!` map, generalizing
//! [`ActivityController::instance`](crate::features::activity::ActivityController::instance)'s
//! single-slot thread-local singleton to one slot per channel): the first
//! screen to visit a channel constructs its controller and kicks the initial
//! [`load`](MessagesController::load) once, and every later visit — from
//! [`crate::screens::channel_feed`] or [`crate::screens::thread`], in either
//! order — reuses that same live `Arc`, with no reload and no second
//! skeleton. This is what makes a reply composed in an open thread show up on
//! an already-open feed's "N replies" affordance (and a message sent in the
//! feed visible the moment its thread opens) — both screens read and write
//! the exact same signals, not independent copies (task 19 closed this gap;
//! `screens::channel_feed` and `screens::thread`'s own module docs cover how
//! each screen now sources its instance). Construct a controller directly
//! ([`MessagesController::new`] / [`with_latency`](MessagesController::with_latency))
//! only for a test that wants its own isolated instance — every screen should
//! go through `for_channel`.
//!
//! `for_channel` kicks its one-time initial load via
//! [`frust::spawn_local`] rather than [`frust::spawn`] (the
//! `Send`-background executor [`load`](MessagesController::load) itself is
//! written against, and still uses for a later user-triggered op like `send`'s
//! canned reply or `#firehose`'s pagination): a construct-then-immediately-
//! read-back sequence — `for_channel` returning straight into the calling
//! screen's own render reading `loading`/`messages` moments later — raced a
//! background-thread write against that same-frame tracked read often enough
//! to intermittently panic ("already disposed") under `cargo test`, since a
//! registry hit now makes that exact sequence run on *every* visit to an
//! already-loaded channel, not just a fresh mount. `spawn_local` (queued on
//! the UI-thread-local task queue, run only when a shell/test pumps it —
//! [`ActivityController::instance`]'s own mount-time kickoff uses the same)
//! keeps that first write on the same thread as the read, closing the race.
//!
//! # Public API — designed with the thread view in mind
//!
//! `screens/thread.rs` consumes this same controller (the `/channel/:id` →
//! `/thread/:id` flow); its needs shaped this module's surface, so the
//! thread screen never touches the mock layer directly:
//!
//! - a **per-channel message list** — [`MessagesController::messages`]
//!   (`RwSignal<Vec<FeedMessage>>`), newest last;
//! - **per-message replies** — each [`FeedMessage::replies`] carries its thread;
//!   [`MessagesController::message`] fetches one parent by id and
//!   [`MessagesController::replies_for`] its thread;
//! - a [`ReplyInThread`](MessagesController::reply_in_thread) op appending to a
//!   message's thread (the thread count [`FeedMessage::reply_count`] the bubble
//!   shows updates in place);
//! - **reaction toggling** — [`MessagesController::toggle_reaction`] flips the
//!   current user's reaction and its count.
//!
//! # State lives in signals (the facade-only rule)
//!
//! Every mutable field is an `RwSignal`/atomic so a background op (a mock load,
//! a canned-reply timer) can write it and wake the shell — the app depends on
//! the `frust` facade alone and never on `reactive_graph` directly (see
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions). The async ops
//! sleep via `clean_signals::time::sleep` (the one sanctioned timer) and are
//! driven off the screen through `frust::spawn`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clean_signals::{ControllerCore, RunOptions, UseCase};
use frust::{Get, GetUntracked, RwSignal, Set, Update};

use crate::failure::HuddleFailure;
use crate::mock;

pub mod domain;

/// How many messages a page holds — the newest page loaded first, and the size
/// of each older page [`load_older`](MessagesController::load_older) prepends.
pub const PAGE_SIZE: usize = 30;

/// Default mock load/pagination latency (skeletons show for roughly this long).
pub const DEFAULT_LOAD_LATENCY: Duration = Duration::from_millis(650);
/// Default delay before a canned reply starts "being typed" after a send.
pub const DEFAULT_REPLY_DELAY: Duration = Duration::from_millis(900);
/// Default duration the typing indicator shows before the canned reply lands.
pub const DEFAULT_TYPING_DURATION: Duration = Duration::from_millis(700);

/// The base of the id space runtime-authored messages (sends + canned replies)
/// draw from, chosen above both the authored ids (1..=40) and the firehose
/// generator's (1000..1120) so a new message never collides with mock data.
const RUNTIME_ID_BASE: u32 = 1_000_000;

/// The body of a [`FeedMessage`] — the owned counterpart to
/// [`mock::MessageBody`] (a sent message carries a runtime `String`, not the
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
    fn from_mock(body: mock::MessageBody) -> Self {
        match body {
            mock::MessageBody::Text(t) => FeedBody::Text(t.to_string()),
            mock::MessageBody::Link { url, title } => FeedBody::Link {
                url: url.to_string(),
                title: title.to_string(),
            },
            mock::MessageBody::File { name, size } => FeedBody::File {
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
    /// Whether [`mock::CURRENT_USER_ID`] is one of them (drives the chip's
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
/// [`mock::Message`] (reactions toggle, a thread grows), plus the runtime
/// messages a send/canned-reply appends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedMessage {
    /// Stable id (mock ids for loaded messages, [`RUNTIME_ID_BASE`]+ for sent).
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
    fn from_mock(m: mock::Message) -> Self {
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
    fn own_text(id: u32, text: String) -> Self {
        FeedMessage {
            id,
            author_id: mock::CURRENT_USER_ID,
            body: FeedBody::Text(text),
            reactions: Vec::new(),
            replies: Vec::new(),
        }
    }

    /// The canned reply the mock backend "types" back after a send.
    fn canned_reply(id: u32) -> Self {
        FeedMessage {
            id,
            // Grace Hopper stands in for the channel's other members.
            author_id: 2,
            body: FeedBody::Text("Got it — thanks for the update!".to_string()),
            reactions: Vec::new(),
            replies: Vec::new(),
        }
    }

    /// Whether this message was authored by [`mock::CURRENT_USER_ID`] (own
    /// messages render right-aligned).
    pub fn is_own(&self) -> bool {
        self.author_id == mock::CURRENT_USER_ID
    }

    /// The thread reply count shown on the bubble.
    pub fn reply_count(&self) -> usize {
        self.replies.len()
    }

    /// Toggle the current user's reaction with `emoji`: add it (count 1) if
    /// absent, join an existing one (+1), or leave one already joined (-1,
    /// removing the chip at zero).
    fn toggle_reaction(&mut self, emoji: &str) {
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

/// Parameters for [`LoadMessages`]: the channel to load, an optional
/// newest-page cap (the firehose loads its newest [`PAGE_SIZE`]), and the mock
/// latency to simulate.
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

/// The message-load use case: sleep the mock latency, then materialize the
/// channel's [`FeedMessage`]s (the newest `limit` of them). Never fails — it
/// reuses [`HuddleFailure`] only to satisfy the [`UseCase`] bound.
pub struct LoadMessages;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for LoadMessages {
    type Params = LoadMessagesParams;
    type Output = Vec<FeedMessage>;
    type Failure = HuddleFailure;

    async fn execute(&self, params: LoadMessagesParams) -> Result<Vec<FeedMessage>, HuddleFailure> {
        clean_signals::time::sleep(params.latency).await;
        let all: Vec<FeedMessage> = mock::messages_for(&params.channel_id)
            .into_iter()
            .map(FeedMessage::from_mock)
            .collect();
        let out = match params.limit {
            Some(n) if all.len() > n => all[all.len() - n..].to_vec(),
            _ => all,
        };
        Ok(out)
    }
}

/// The channel-feed view model. One per channel id, constructed by the screen.
///
/// See the [module docs](self) for the thread-view-shaped public surface.
pub struct MessagesController {
    core: ControllerCore<HuddleFailure>,
    load_messages: LoadMessages,
    channel_id: String,
    load_latency: Duration,
    reply_delay: Duration,
    typing_duration: Duration,
    /// The next id a runtime-authored message (send / canned reply) takes.
    next_id: AtomicU32,
    /// The index into the channel's full (oldest-first) source of the first
    /// currently-shown message — the pagination cursor `load_older` walks down.
    oldest_index: AtomicU32,

    /// The loaded messages, oldest-first (newest at the bottom of the feed).
    pub messages: RwSignal<Vec<FeedMessage>>,
    /// Whether the initial [`LoadMessages`] is in flight (drives skeletons).
    pub loading: RwSignal<bool>,
    /// Whether an older page is being fetched (drives the "loading older…" row).
    pub loading_older: RwSignal<bool>,
    /// Whether the mock reply is currently "being typed" (drives the typing
    /// indicator).
    pub typing: RwSignal<bool>,
    /// Whether an older page exists (only `#firehose` ever has more).
    pub has_more: RwSignal<bool>,
}

impl MessagesController {
    /// A controller for `channel_id` with the default mock latencies.
    pub fn new(channel_id: impl Into<String>) -> Self {
        Self::with_latency(
            channel_id,
            DEFAULT_LOAD_LATENCY,
            DEFAULT_REPLY_DELAY,
            DEFAULT_TYPING_DURATION,
        )
    }

    /// A controller with explicit latencies — the headless suite passes short
    /// durations so a load / send / reply round trip completes fast.
    pub fn with_latency(
        channel_id: impl Into<String>,
        load_latency: Duration,
        reply_delay: Duration,
        typing_duration: Duration,
    ) -> Self {
        Self {
            core: ControllerCore::new(),
            load_messages: LoadMessages,
            channel_id: channel_id.into(),
            load_latency,
            reply_delay,
            typing_duration,
            next_id: AtomicU32::new(RUNTIME_ID_BASE),
            oldest_index: AtomicU32::new(0),
            messages: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            loading_older: RwSignal::new(false),
            typing: RwSignal::new(false),
            has_more: RwSignal::new(false),
        }
    }

    /// The single shared instance for `channel_id` — see the [module
    /// docs](self)' "One shared controller per channel" section. Get-or-create:
    /// the first call for a given `channel_id` constructs a fresh controller
    /// with the default latencies and kicks its initial [`load`](Self::load)
    /// once (the skeleton a screen's first visit shows); a later call for the
    /// same `channel_id` — from either `channel_feed` or `thread`, in either
    /// order — returns the same `Arc` with no reload.
    pub fn for_channel(channel_id: impl Into<String>) -> Arc<Self> {
        thread_local! {
            static REGISTRY: RefCell<HashMap<String, Arc<MessagesController>>> =
                RefCell::new(HashMap::new());
        }
        let channel_id = channel_id.into();
        REGISTRY.with(|cell| {
            // Self-heal (task 22 hardening): a cached controller's signals are
            // owned by the reactive `Owner` live when it was built; a headless
            // test that disposes its owner and re-enters (or a reused test
            // thread) leaves this registry holding a controller whose signals
            // are disposed, so the next `loading`/`messages` read panics.
            // Probing one signal with `try_get_untracked` detects that, and a
            // disposed hit is rebuilt (and its load re-kicked) rather than
            // handed back — the same guard the composer/sheet caches now carry.
            if let Some(existing) = cell.borrow().get(&channel_id).cloned()
                && existing.loading.try_get_untracked().is_some()
            {
                return existing;
            }
            let controller = Arc::new(MessagesController::new(channel_id.clone()));
            {
                let handle = Arc::clone(&controller);
                frust::spawn_local(async move {
                    handle.load().await;
                });
            }
            cell.borrow_mut()
                .insert(channel_id, Arc::clone(&controller));
            controller
        })
    }

    /// The channel/DM this controller feeds.
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    fn mint_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Load the newest page of messages. `#firehose` loads only its newest
    /// [`PAGE_SIZE`] (the rest arrive via [`load_older`](Self::load_older));
    /// every other channel loads whole. Routed through [`ControllerCore::run`]
    /// so the clean-architecture spine stays visible.
    pub async fn load(&self) {
        self.loading.set(true);
        let is_firehose = self.channel_id == mock::FIREHOSE_ID;
        let limit = is_firehose.then_some(PAGE_SIZE);
        let params = LoadMessagesParams {
            channel_id: self.channel_id.clone(),
            limit,
            latency: self.load_latency,
        };
        let result = self
            .core
            .run(&self.load_messages, params, RunOptions::default())
            .await;
        if let Ok(msgs) = result {
            let total = mock::messages_for(&self.channel_id).len();
            let shown = msgs.len();
            let oldest = total.saturating_sub(shown);
            self.oldest_index.store(oldest as u32, Ordering::Relaxed);
            self.has_more.set(is_firehose && oldest > 0);
            self.messages.set(msgs);
        }
        self.loading.set(false);
    }

    /// Prepend the next older [`PAGE_SIZE`] page to the feed (only `#firehose`
    /// has more). A no-op when nothing older remains or a page is already in
    /// flight — the near-start infinite-scroll trigger.
    pub async fn load_older(&self) {
        if !self.has_more.get_untracked() || self.loading_older.get_untracked() {
            return;
        }
        self.loading_older.set(true);
        clean_signals::time::sleep(self.load_latency).await;

        let source: Vec<FeedMessage> = mock::messages_for(&self.channel_id)
            .into_iter()
            .map(FeedMessage::from_mock)
            .collect();
        let old_start = self.oldest_index.load(Ordering::Relaxed) as usize;
        let new_start = old_start.saturating_sub(PAGE_SIZE);
        let older = source[new_start..old_start].to_vec();

        self.messages.update(|v| {
            let mut combined = older;
            combined.append(v);
            *v = combined;
        });
        self.oldest_index.store(new_start as u32, Ordering::Relaxed);
        self.has_more.set(new_start > 0);
        self.loading_older.set(false);
    }

    /// Append the current user's message to the feed immediately (the composer
    /// clears the instant this returns). Returns the new message's id.
    pub fn send_now(&self, text: impl Into<String>) -> u32 {
        let id = self.mint_id();
        self.messages
            .update(|v| v.push(FeedMessage::own_text(id, text.into())));
        id
    }

    /// After a send, drive the canned reply: wait the reply delay, show the
    /// typing indicator for its duration, then append the reply. Spawned off
    /// the screen via `frust::spawn`.
    pub async fn deliver_reply(&self) {
        clean_signals::time::sleep(self.reply_delay).await;
        self.typing.set(true);
        clean_signals::time::sleep(self.typing_duration).await;
        self.typing.set(false);
        let id = self.mint_id();
        self.messages
            .update(|v| v.push(FeedMessage::canned_reply(id)));
    }

    /// Toggle the current user's `emoji` reaction on message `msg_id`.
    pub fn toggle_reaction(&self, msg_id: u32, emoji: &str) {
        self.messages.update(|v| {
            if let Some(m) = v.iter_mut().find(|m| m.id == msg_id) {
                m.toggle_reaction(emoji);
            }
        });
    }

    /// Append `text` to message `msg_id`'s thread (the `ReplyInThread` op the
    /// thread screen drives). The bubble's [`FeedMessage::reply_count`] updates
    /// in place.
    pub fn reply_in_thread(&self, msg_id: u32, text: impl Into<String>) {
        let text = text.into();
        self.messages.update(|v| {
            if let Some(m) = v.iter_mut().find(|m| m.id == msg_id) {
                m.replies.push(FeedReply {
                    author_id: mock::CURRENT_USER_ID,
                    text,
                });
            }
        });
    }

    /// One message by id (the thread screen's parent lookup), read untracked.
    pub fn message(&self, msg_id: u32) -> Option<FeedMessage> {
        self.messages
            .get_untracked()
            .into_iter()
            .find(|m| m.id == msg_id)
    }

    /// A message's thread, read untracked (the thread screen's reply list).
    pub fn replies_for(&self, msg_id: u32) -> Vec<FeedReply> {
        self.message(msg_id).map(|m| m.replies).unwrap_or_default()
    }

    /// Snapshot of the feed, read without tracking — for tests/inspection.
    pub fn snapshot(&self) -> Vec<FeedMessage> {
        self.messages.get_untracked()
    }

    /// The feed, tracked — drives the screen's rebuild.
    pub fn feed(&self) -> Vec<FeedMessage> {
        self.messages.get()
    }
}

impl AsRef<ControllerCore<HuddleFailure>> for MessagesController {
    fn as_ref(&self) -> &ControllerCore<HuddleFailure> {
        &self.core
    }
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
        assert!(text_message(1, mock::CURRENT_USER_ID).is_own());
        assert!(!text_message(1, 2).is_own());
    }

    #[test]
    fn from_mock_seeds_a_thread_on_the_deterministic_subset() {
        let threaded = FeedMessage::from_mock(mock::Message {
            id: 4,
            channel_id: "general",
            author_id: 2,
            body: mock::MessageBody::Text("x"),
            reactions: &[],
        });
        assert_eq!(threaded.reply_count(), 2, "id % 4 == 0 seeds a thread");

        let plain = FeedMessage::from_mock(mock::Message {
            id: 5,
            channel_id: "general",
            author_id: 2,
            body: mock::MessageBody::Text("x"),
            reactions: &[],
        });
        assert_eq!(plain.reply_count(), 0);
    }
}
