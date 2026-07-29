//! [`MessagesController`] — the channel/DM feed view model (moved verbatim
//! apart from repo threading from the former flat
//! `features/messages/mod.rs`).
//!
//! It owns every signal the feed and thread pages render and drives the
//! mock-latency load / send / react / thread-reply / pagination operations
//! through an embedded [`ControllerCore`] — the same controller-embeds-
//! `ControllerCore` shape as [`crate::features::settings`].
//!
//! # One shared controller per channel
//!
//! [`MessagesController::for_channel`] is a per-`channel_id` registry (a
//! `thread_local!` map, generalizing
//! [`ActivityController::instance`](crate::features::activity::ActivityController::instance)'s
//! single-slot thread-local singleton to one slot per channel): the first
//! screen to visit a channel constructs its controller and kicks the initial
//! [`load`](MessagesController::load) once, and every later visit — from
//! [`channel_feed`](super::pages::channel_feed) or [`thread`](super::pages::thread),
//! in either order — reuses that same live `Arc`, with no reload and no second
//! skeleton. This is what makes a reply composed in an open thread show up on
//! an already-open feed's "N replies" affordance (and a message sent in the
//! feed visible the moment its thread opens) — both screens read and write
//! the exact same signals, not independent copies. Construct a controller
//! directly ([`MessagesController::new`] /
//! [`with_latency`](MessagesController::with_latency)) only for a test that
//! wants its own isolated instance — every screen should go through
//! `for_channel`.
//!
//! # Repository injection (the `for_channel` path)
//!
//! Every constructor resolves the [`MessageRepository`] via
//! [`resolve_repo`] — the injected `Arc<dyn MessageRepository + Send + Sync>`
//! the composition root (`crate::HuddleApp::init`) `provide_context`s, or, when
//! no context is in scope (a standalone controller-unit test that never mounts
//! the app), a freshly-constructed `StoreMessageRepository`. Because
//! `for_channel`/`new`/`with_latency` keep their exact pre-refactor signatures
//! (channel id only), this internal resolution is what lets the injection reach
//! every construction site — the `nav.push` pages AND the integration tests that
//! call `for_channel`/`with_latency` — with **zero** call-site edits.
//!
//! `for_channel` kicks its one-time initial load via [`frust::spawn_local`]
//! rather than [`frust::spawn`] so the first `loading`/`messages` write lands on
//! the same (UI) thread the calling screen's same-frame read is on, closing the
//! "already disposed" race a registry hit would otherwise open on every revisit.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clean_signals::{ControllerCore, RunOptions};
use frust::{Get, GetUntracked, RwSignal, Set, Update, use_context};

use crate::failure::HuddleFailure;
use crate::features::channels::domain::{Channel, Dm};
use crate::features::messages::data::repositories::StoreMessageRepository;
use crate::features::messages::domain::models::{
    FeedMessage, FeedReply, LoadMessagesParams, PAGE_SIZE,
};
use crate::features::messages::domain::repositories::MessageRepository;
use crate::features::messages::domain::use_cases::LoadMessages;
use crate::features::profile::domain::{CURRENT_USER_ID, User};

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

/// Resolve the message repository at construction time: the injected
/// `Arc<dyn MessageRepository + Send + Sync>` the composition root published via
/// `provide_context`, or — when no context is in scope (a standalone
/// controller-unit test) — a fresh `StoreMessageRepository`. This fallback is
/// the messages feature's composition-root equivalent for context-less
/// construction (the same shape as a `#[cfg(test)]`-only `store_repo()`
/// helper elsewhere in this crate), forced into production here because
/// `for_channel`/`with_latency` are public constructors the external
/// integration-test crates call with a channel id only (behavior-preserving:
/// injected and fallback are the same `StoreMessageRepository`, so the data
/// — and behavior — are identical either way). The one presentation→data
/// reference in the slice, allowlisted as such by the crate's
/// architecture-conformance test.
fn resolve_repo() -> Arc<dyn MessageRepository + Send + Sync> {
    use_context::<Arc<dyn MessageRepository + Send + Sync>>()
        .unwrap_or_else(|| Arc::new(StoreMessageRepository::new()))
}

/// The channel-feed view model. One per channel id, constructed by the screen.
///
/// See the [module docs](self) for the thread-view-shaped public surface.
pub struct MessagesController {
    core: ControllerCore<HuddleFailure>,
    repo: Arc<dyn MessageRepository + Send + Sync>,
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
        let repo = resolve_repo();
        Self {
            core: ControllerCore::new(),
            load_messages: LoadMessages::new(Arc::clone(&repo)),
            repo,
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
            // Self-heal: a cached controller's signals are
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

    /// Look up a channel by id (the feed/thread app-bar title) — through the
    /// injected [`MessageRepository`].
    pub fn channel(&self, id: &str) -> Option<Channel> {
        self.repo.channel(id)
    }

    /// All direct-message conversations (the app-bar's DM-peer derivation).
    pub fn dms(&self) -> Vec<Dm> {
        self.repo.dms()
    }

    /// All workspace members (the app-bar's member count).
    pub fn users(&self) -> Vec<User> {
        self.repo.users()
    }

    /// Look up a member by id (avatar initials / author names).
    pub fn user(&self, id: u32) -> Option<User> {
        self.repo.user(id)
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
        let is_firehose = self.repo.is_firehose(&self.channel_id);
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
            let total = self
                .repo
                .messages_for(&self.channel_id)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
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

        let source: Vec<FeedMessage> = self
            .repo
            .messages_for(&self.channel_id)
            .await
            .unwrap_or_default()
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
                    author_id: CURRENT_USER_ID,
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
