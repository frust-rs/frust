//! TEA engine: model ([`AppState`]), messages ([`Message`]), the pure
//! transition ([`update`]), and the [`Engine`] that owns the model plus the
//! unified message channel background tasks feed.
//!
//! Layering (D2): this module never touches the terminal, ratatui, or any
//! render type — it is the testable core. `crate::ui` renders `&AppState` and
//! emits `Message`s but never mutates the model; `crate::runner` owns the
//! terminal lifecycle and drives the loop.

mod message;
mod state;
mod update;

pub use message::{Message, RegionId};
pub use state::{AppState, CREATE_TOAST, Screen};
pub use update::{Outcome, update};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Owns the model and the single mpsc channel every asynchronous producer
/// (Phase 2 session supervisors, preflight tasks) sends into.
///
/// The channel is unbounded and cloneable: [`Engine::sender`] hands a producer
/// a `Sender`, and the event loop drains the matching receiver
/// ([`Engine::take_receiver`]) in its `select!`. Mutation flows through exactly
/// one method — [`Engine::handle`] — which forwards to the pure [`update`].
pub struct Engine {
    /// The application model.
    pub state: AppState,
    tx: UnboundedSender<Message>,
    rx: Option<UnboundedReceiver<Message>>,
}

impl Engine {
    /// Wrap a model in an engine, creating its message channel.
    pub fn new(state: AppState) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            state,
            tx,
            rx: Some(rx),
        }
    }

    /// A cloneable handle a background task posts `Message`s through.
    pub fn sender(&self) -> UnboundedSender<Message> {
        self.tx.clone()
    }

    /// Take the receiver for the event loop. Callable once; the loop selects on
    /// it. Panics if called twice (a wiring bug, not a runtime condition).
    pub fn take_receiver(&mut self) -> UnboundedReceiver<Message> {
        self.rx
            .take()
            .expect("Engine::take_receiver called more than once")
    }

    /// The single mutation point: apply one message to the model.
    pub fn handle(&mut self, msg: Message) -> Outcome {
        update(&mut self.state, msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_routes_through_update() {
        let mut engine = Engine::new(AppState::default());
        let out = engine.handle(Message::Quit);
        assert!(engine.state.should_quit);
        assert!(!out.redraw);
    }

    #[tokio::test]
    async fn channel_delivers_to_receiver() {
        let mut engine = Engine::new(AppState::default());
        let mut rx = engine.take_receiver();
        engine.sender().send(Message::Tick).unwrap();
        assert_eq!(rx.recv().await, Some(Message::Tick));
    }
}
