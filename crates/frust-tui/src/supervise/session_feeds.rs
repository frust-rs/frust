//! The two deferred-answer registries the embedded servers' backend seam
//! needs: live session-event subscriptions, and in-flight widget-tree pulls.
//!
//! Both exist for the same reason. `frust-mcp`'s `SessionBackend` is sync and
//! answer-shaped, but two of its fourteen methods cannot be answered from one
//! read of `AppState`:
//!
//! - [`SessionBackend::subscribe_session_events`](frust_mcp::SessionBackend::subscribe_session_events)
//!   hands back a *feed* that must keep delivering long after the call
//!   returns — every subsequent log line, then the session's end.
//! - [`SessionBackend::fetch_widget_tree`](frust_mcp::SessionBackend::fetch_widget_tree)
//!   needs a round trip over a devtools connection the workbench owns inside
//!   `crate::supervise::DevtoolsBridge`'s own thread, which reports back
//!   asynchronously as a [`Message::DevtoolsInspector`].
//!
//! So the workbench keeps the *sending* halves here, beside the
//! [`crate::supervise::McpSessionRecords`] the same seam already needs, and
//! `crate::runner` drives them from the event loop.
//!
//! # Why the fan-out is the runner's, not `update`'s
//!
//! Pushing a line onto a subscriber's channel is I/O-shaped work, and
//! `crate::engine::update` is pure by contract
//! (`docs/TUI_ARCHITECTURE.md`'s layering). The runner therefore takes a
//! [`SessionCursor`] *before* handing a `Message::Session` to `update`, and
//! replays whatever the transition appended (and whether it ended the
//! session) *after* — reading the model rather than intercepting the write.
//!
//! That ordering is what makes the feed's redaction guarantee free: the lines
//! it replays are the ones `SessionView::push_line_at` actually stored, which
//! are already token-redacted (`docs/DEVTOOLS_ARCHITECTURE.md`), so a DAP
//! client's Debug Console can no more read a devtools handshake token back
//! than the log pane can.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use frust_devtools_protocol::serde_json;
use frust_mcp::engine::{
    LOG_SUBSCRIPTION_CAP, SessionEventFeed, SessionEventSink, SessionState as McpSessionState,
};

use super::SessionId;
use super::mcp_backend::{Reply, TreeRefusal};
use crate::engine::{AppState, InspectorEvent, SessionView};

/// How long an unanswered widget-tree request is kept before it is swept.
///
/// Deliberately the backend's own reply deadline
/// (`crate::supervise::mcp_backend`'s `REPLY_DEADLINE`): past it the caller
/// has already given up and taken its typed `WorkbenchUnreachable`, so the
/// entry can only ever answer nobody. Kept as a separate constant rather than
/// shared, because the two are the same number for a reason that is not a
/// dependency — one bounds a caller's wait, this one bounds a map.
const TREE_REQUEST_TTL: Duration = Duration::from_secs(5);

// ── Live session-event subscriptions ────────────────────────────────────────

/// Where a session's view stood *before* one `Message::Session` was applied —
/// everything the fan-out below needs to say what that transition added.
///
/// Taken by `crate::runner` immediately before `update` runs, and consumed
/// immediately after; it names no borrow, so it can straddle the `&mut
/// AppState` the transition itself takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCursor {
    /// The session the message was for.
    pub session: SessionId,
    /// The absolute index the session's log ended at — the first line the
    /// transition could have appended.
    pub next_line: u64,
    /// Whether the session had *already* reached a terminal state, so a
    /// session that is terminal afterwards is only reported as newly ended
    /// when this is `false` (a repeated terminal report must not send a
    /// second `Exited`).
    pub was_terminal: bool,
}

/// Every open [`SessionEventFeed`]'s sending half, keyed by session.
///
/// One subscriber per session, exactly like `frust-mcp`'s own engine: a
/// second [`subscribe`](Self::subscribe) replaces the first, whose consumer
/// then sees its feed close. Owned by `crate::runner` for the workbench's
/// whole life; dropping it (at quit) closes every feed, which is what stops a
/// DAP client's log pump from waiting on a workbench that is gone.
#[derive(Default)]
pub struct SessionSubscribers {
    by_session: HashMap<SessionId, SessionEventSink>,
}

impl SessionSubscribers {
    /// No subscribers.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many sessions currently have a subscriber — diagnostics and tests
    /// only; nothing renders it.
    pub fn len(&self) -> usize {
        self.by_session.len()
    }

    /// Whether no session has a subscriber.
    pub fn is_empty(&self) -> bool {
        self.by_session.is_empty()
    }

    /// Opens a feed over `view`, seeded with its retained log and registered
    /// for live delivery.
    ///
    /// `terminal` is the session's `frust-mcp` state **if it has already
    /// ended**. A session that ended keeps its seed and is handed its
    /// [`SessionEvent::Exited`](frust_mcp::engine::SessionEvent::Exited)
    /// immediately rather than being registered for a transition that will
    /// never come again — the same choice `frust-mcp`'s `subscribe_events`
    /// makes, and the reason a consumer never hangs on a session that is
    /// already over.
    ///
    /// A backlog deeper than the feed's log budget is seeded with its newest
    /// lines behind one in-band marker naming what was skipped.
    pub fn subscribe(
        &mut self,
        view: &SessionView,
        terminal: Option<McpSessionState>,
    ) -> SessionEventFeed {
        let (mut sink, feed) = SessionEventFeed::channel();
        let held = view.log.len();
        let seed_from = if held > LOG_SUBSCRIPTION_CAP {
            // One slot goes to the marker, so the seed is the newest
            // `cap - 1` lines and the log budget opens exactly full.
            let kept = LOG_SUBSCRIPTION_CAP - 1;
            sink.note_backlog_skipped((held - kept) as u64);
            view.log.end_index().saturating_sub(kept as u64)
        } else {
            view.log.base_index()
        };
        for (abs, line) in view.log.iter() {
            if abs >= seed_from {
                sink.offer(line);
            }
        }
        match terminal {
            Some(state) => sink.finish(state),
            None => {
                self.by_session.insert(view.id, sink);
            }
        }
        feed
    }

    /// The pre-transition cursor for `session`, or `None` when the workbench
    /// holds no view for it (nothing to replay from) — see [`SessionCursor`].
    pub fn cursor(state: &AppState, session: SessionId) -> Option<SessionCursor> {
        let view = state.sessions.iter().find(|view| view.id == session)?;
        Some(SessionCursor {
            session,
            next_line: view.log.end_index(),
            was_terminal: view.state.is_terminal(),
        })
    }

    /// Replay whatever the just-applied transition added to `cursor`'s
    /// session: every log line appended since, and — when the session ended
    /// on this transition — its [`SessionEvent::Exited`](frust_mcp::engine::SessionEvent::Exited).
    ///
    /// `terminal_state` is the session's `frust-mcp` state, computed by the
    /// caller (which owns the launch records a `failed` state is derived
    /// from) and used only if the session is newly terminal.
    pub fn replay(
        &mut self,
        state: &AppState,
        cursor: SessionCursor,
        terminal_state: impl FnOnce(&SessionView) -> McpSessionState,
    ) {
        let Some(view) = state.sessions.iter().find(|view| view.id == cursor.session) else {
            // The view is gone: the subscriber is owed an ending it can no
            // longer be given honestly, so close its feed instead of leaving
            // it waiting forever. (No tab-close path exists in the workbench
            // today, so this is defensive rather than reachable.)
            self.by_session.remove(&cursor.session);
            return;
        };
        if self.by_session.contains_key(&view.id) {
            self.deliver_lines(view, cursor.next_line);
        }
        if !cursor.was_terminal && view.state.is_terminal() {
            self.finish(view.id, terminal_state(view));
        }
    }

    /// Close `session`'s feed with its terminal state. A session with no
    /// subscriber is a no-op.
    pub fn finish(&mut self, session: SessionId, state: McpSessionState) {
        if let Some(sink) = self.by_session.remove(&session) {
            sink.finish(state);
        }
    }

    /// Offer every line `view` gained since absolute index `from`.
    ///
    /// A `from` older than the ring's own base means eviction outran the
    /// replay (only reachable for a single batch deeper than the 10 000-line
    /// ring); the gap is reported in band rather than passed over silently.
    fn deliver_lines(&mut self, view: &SessionView, from: u64) {
        let Some(sink) = self.by_session.get_mut(&view.id) else {
            return;
        };
        let base = view.log.base_index();
        if base > from {
            sink.note_backlog_skipped(base - from);
        }
        let mut alive = true;
        for abs in from.max(base)..view.log.end_index() {
            let Some(line) = view.log.get(abs) else {
                continue;
            };
            if !sink.offer(line) {
                alive = false;
                break;
            }
        }
        if !alive {
            // The consumer dropped its feed; clear the slot so the next
            // transition does not retry a dead channel.
            self.by_session.remove(&view.id);
        }
    }
}

// ── In-flight widget-tree pulls ─────────────────────────────────────────────

/// One backend caller waiting for a `widget_tree` pull to come back.
struct PendingTree {
    /// When the wait started — see [`TREE_REQUEST_TTL`].
    since: Instant,
    /// Where the answer goes.
    reply: Reply<Result<serde_json::Value, TreeRefusal>>,
}

/// Widget-tree requests the workbench has issued to a session's
/// [`DevtoolsBridge`](crate::supervise::DevtoolsBridge) and not yet answered.
///
/// The bridge owns the only devtools socket, serves a pull on its own thread,
/// and reports the result back as a [`Message::DevtoolsInspector`] — so a
/// backend caller's answer is necessarily *deferred*. It blocks meanwhile
/// (bounded by `crate::supervise::mcp_backend`'s reply deadline), which is
/// what makes replying from the completion path a legitimate answer rather
/// than a lie: what it gets is a **freshly pulled tree**, not a cached one.
#[derive(Default)]
pub struct PendingWidgetTrees {
    by_session: HashMap<SessionId, Vec<PendingTree>>,
}

impl PendingWidgetTrees {
    /// No requests in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one caller waiting on `session`'s next tree, sweeping any whose
    /// caller has already timed out.
    ///
    /// Several callers may wait at once (two DAP clients, or a DAP client and
    /// an agent): they share the one pull, since a second bridge request would
    /// only make the first one slower.
    pub fn push(
        &mut self,
        session: SessionId,
        reply: Reply<Result<serde_json::Value, TreeRefusal>>,
    ) {
        self.sweep();
        self.by_session
            .entry(session)
            .or_default()
            .push(PendingTree {
                since: Instant::now(),
                reply,
            });
    }

    /// Answer everyone waiting on `session` from one bridge report.
    ///
    /// A `PropsArrived` is somebody else's answer (the Inspector's
    /// per-selection pull) and leaves the waiters in place. Returns whether
    /// anything was answered — tests read it; the runner does not care.
    pub fn resolve(&mut self, session: SessionId, event: &InspectorEvent) -> bool {
        let answer = match event {
            InspectorEvent::TreeArrived(dump) => match serde_json::to_value(dump) {
                Ok(value) => Ok(value),
                Err(err) => Err(TreeRefusal::Failed(format!(
                    "serializing the widget tree: {err}"
                ))),
            },
            InspectorEvent::Failed(error) => Err(TreeRefusal::Failed(error.clone())),
            InspectorEvent::PropsArrived(..) => return false,
        };
        let Some(waiting) = self.by_session.remove(&session) else {
            return false;
        };
        for pending in &waiting {
            pending.reply.send(answer.clone());
        }
        !waiting.is_empty()
    }

    /// Answer everyone waiting on `session` with a refusal — the session
    /// ended, or its devtools connection went away, so no pull is coming.
    pub fn refuse(&mut self, session: SessionId, why: &str) {
        if let Some(waiting) = self.by_session.remove(&session) {
            for pending in waiting {
                pending
                    .reply
                    .send(Err(TreeRefusal::Unavailable(why.to_string())));
            }
        }
    }

    /// How many sessions have a request in flight — tests only.
    pub fn len(&self) -> usize {
        self.by_session.len()
    }

    /// Whether nothing is in flight.
    pub fn is_empty(&self) -> bool {
        self.by_session.is_empty()
    }

    /// Drop entries whose caller has already stopped waiting. Answering a
    /// [`Reply`] nobody holds is harmless (it is a no-op), so this bounds
    /// memory rather than correctness.
    fn sweep(&mut self) {
        let now = Instant::now();
        self.by_session.retain(|_, waiting| {
            waiting.retain(|pending| now.duration_since(pending.since) < TREE_REQUEST_TTL);
            !waiting.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc::TryRecvError;
    use std::time::Duration;

    use frust_devtools_protocol::{WidgetNode, WidgetTreeDump};
    use frust_mcp::engine::SessionEvent;

    use super::*;
    use crate::engine::SessionView;
    use crate::supervise::SessionState;

    /// A generous failure deadline — never a pacing device.
    const RECV: Duration = Duration::from_secs(5);

    fn view(id: u64) -> SessionView {
        SessionView::new(
            SessionId(id),
            PathBuf::from("/tmp/frust-tui-feeds"),
            "desktop",
        )
    }

    fn state_with(views: Vec<SessionView>) -> AppState {
        AppState {
            sessions: views,
            ..AppState::default()
        }
    }

    fn exited() -> McpSessionState {
        McpSessionState::Exited { success: true }
    }

    #[test]
    fn a_feed_is_seeded_with_the_retained_log_then_fed_live_then_closed() {
        let mut view = view(0);
        for line in ["one", "two"] {
            view.push_line_at(line.to_string(), "00:00:00");
        }
        let mut subscribers = SessionSubscribers::new();
        let feed = subscribers.subscribe(&view, None);

        // The seed is there before anything else happens.
        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Log("one".to_string()))
        );
        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Log("two".to_string()))
        );

        // A line batch: the runner's cursor is taken first, `update` appends,
        // and the replay delivers exactly what was appended.
        let mut state = state_with(vec![view]);
        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].push_line_at("three".to_string(), "00:00:01");
        subscribers.replay(&state, cursor, |_| exited());
        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Log("three".to_string()))
        );

        // The session ends: the feed gets its exit and then closes.
        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].state = SessionState::Exited(true);
        subscribers.replay(&state, cursor, |_| exited());
        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Exited { state: exited() })
        );
        assert!(
            feed.recv_timeout(RECV).is_err(),
            "the feed closes after the exit it owes"
        );
        assert!(
            subscribers.is_empty(),
            "a finished session's sink is not retained"
        );
    }

    #[test]
    fn subscribing_to_an_already_ended_session_gets_its_seed_and_its_exit_at_once() {
        let mut view = view(3);
        view.push_line_at("last words".to_string(), "00:00:00");
        view.state = SessionState::Exited(false);

        let mut subscribers = SessionSubscribers::new();
        let feed = subscribers.subscribe(&view, Some(McpSessionState::Exited { success: false }));

        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Log("last words".to_string()))
        );
        assert_eq!(
            feed.recv_timeout(RECV),
            Ok(SessionEvent::Exited {
                state: McpSessionState::Exited { success: false }
            })
        );
        assert!(feed.recv_timeout(RECV).is_err(), "and then it closes");
        assert!(
            subscribers.is_empty(),
            "an ended session is never registered for a transition that cannot come"
        );
    }

    #[test]
    fn a_repeated_terminal_report_does_not_send_a_second_exit() {
        let mut view = view(0);
        view.state = SessionState::Exited(true);
        let mut subscribers = SessionSubscribers::new();
        // Registered as if still live, then reported terminal twice.
        let feed = subscribers.subscribe(&view, None);
        let state = state_with(vec![view]);

        let cursor = SessionCursor {
            session: SessionId(0),
            next_line: 0,
            was_terminal: true,
        };
        subscribers.replay(&state, cursor, |_| exited());
        assert_eq!(
            feed.try_recv(),
            Err(TryRecvError::Empty),
            "a session that was already terminal has not just ended"
        );
    }

    #[test]
    fn a_vanished_session_closes_its_feed_rather_than_leaving_it_waiting() {
        let view = view(0);
        let mut subscribers = SessionSubscribers::new();
        let feed = subscribers.subscribe(&view, None);

        // The view is gone from the model entirely.
        let state = state_with(Vec::new());
        subscribers.replay(
            &state,
            SessionCursor {
                session: SessionId(0),
                next_line: 0,
                was_terminal: false,
            },
            |_| exited(),
        );
        assert!(
            feed.recv_timeout(RECV).is_err(),
            "a subscriber must never hang on a session the workbench no longer holds"
        );
    }

    #[test]
    fn a_dropped_feed_clears_its_slot_rather_than_being_retried_forever() {
        let view = view(0);
        let mut subscribers = SessionSubscribers::new();
        let feed = subscribers.subscribe(&view, None);
        drop(feed);

        let mut state = state_with(vec![view]);
        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].push_line_at("nobody is listening".to_string(), "00:00:00");
        subscribers.replay(&state, cursor, |_| exited());

        assert!(
            subscribers.is_empty(),
            "send failure is how a gone consumer is detected"
        );
    }

    #[test]
    fn a_second_subscription_replaces_the_first() {
        let view = view(0);
        let mut subscribers = SessionSubscribers::new();
        let first = subscribers.subscribe(&view, None);
        let _second = subscribers.subscribe(&view, None);
        assert_eq!(subscribers.len(), 1);
        assert!(
            first.recv_timeout(RECV).is_err(),
            "the replaced feed closes rather than silently going quiet"
        );
    }

    // ── Widget-tree pulls ───────────────────────────────────────────────────

    fn dump() -> WidgetTreeDump {
        WidgetTreeDump {
            roots: vec![WidgetNode {
                id: 1,
                type_name: "Root".to_string(),
                debug_label: None,
                bounds: None,
                children: Vec::new(),
            }],
        }
    }

    #[test]
    fn a_pending_tree_request_is_answered_from_the_bridges_report() {
        let mut pending = PendingWidgetTrees::new();
        let (reply, rx) = Reply::channel();
        pending.push(SessionId(0), reply);

        assert!(pending.resolve(SessionId(0), &InspectorEvent::TreeArrived(dump())));
        let answer = rx.recv_timeout(RECV).expect("answered");
        let value = answer.expect("a tree, not a refusal");
        assert_eq!(value["roots"][0]["type_name"], "Root");
        assert!(pending.is_empty());
    }

    #[test]
    fn a_props_report_is_not_a_tree_answer() {
        let mut pending = PendingWidgetTrees::new();
        let (reply, rx) = Reply::channel();
        pending.push(SessionId(0), reply);

        let props = frust_devtools_protocol::WidgetProps {
            id: 1,
            entries: Vec::new(),
        };
        assert!(!pending.resolve(SessionId(0), &InspectorEvent::PropsArrived(1, props)));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(pending.len(), 1, "the tree waiter is still waiting");
    }

    #[test]
    fn a_failed_pull_is_reported_rather_than_left_hanging() {
        let mut pending = PendingWidgetTrees::new();
        let (reply, rx) = Reply::channel();
        pending.push(SessionId(0), reply);

        pending.resolve(
            SessionId(0),
            &InspectorEvent::Failed("connection closed".to_string()),
        );
        let answer = rx.recv_timeout(RECV).expect("answered");
        assert_eq!(
            answer,
            Err(TreeRefusal::Failed("connection closed".to_string()))
        );
    }
}
