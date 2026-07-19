//! [`async_view`] — a snapshot renderer for a controller's [`AsyncState`].
//!
//! This is the Frust transliteration of `clean-signals-leptos`'s
//! `<AsyncView>` component (lifted from Frust's `examples/inbox`, which
//! prototyped it as extractable glue — see that crate's `src/lib.rs`).
//! Leptos re-renders reactively, fragment by fragment, whenever its `state`
//! signal changes; Frust has no per-component fine-grained reactivity (see
//! this crate's rustdoc, "Coarse-grained reactivity model"), so `async_view`
//! is a plain match over an already-read `AsyncState` snapshot, not a
//! reactive wrapper.
//!
//! Take the snapshot with a tracked `signal.get()` at the call site (inside
//! [`Component::build`](frust::Component::build)) so the shell's
//! frame-tracking scope re-renders when the controller later writes a new
//! state.

use clean_signals::{AsyncState, Failure};
use frust::AnyView;

/// Maps a snapshot of an [`AsyncState`] to a view.
///
/// - **`Loading`** → `loading()` (a first-load placeholder — no value has
///   ever loaded yet).
/// - **`Data(v)`** → `data(v)`.
/// - **`Reloading(v)`** → `data(v)` — the stale value stays visible while a
///   refresh is in flight, exactly like `Data`. `async_view` does not
///   distinguish the two arms; if a caller needs a "refreshing" indicator, gate
///   it on the raw `AsyncState` before calling `async_view` (see Future
///   Enhancements below).
/// - **`Error { failure, stale }`** → `error(failure)`. **`stale` is dropped
///   in this minimal v0 shape** — a failed (re)fetch always renders the error
///   slot alone, even if a previously-loaded value is available. A future
///   revision may add an optional "error-with-stale-data" slot; until then,
///   callers that need the last-known-good value alongside the error should
///   match on the raw `AsyncState` themselves rather than go through
///   `async_view`.
///
/// # Future Enhancements (deferred, not implemented here)
///
/// `clean-signals-leptos`'s `<AsyncView>` additionally offers optional
/// `reloading_indicator` and default loading/error slots (falling back to a
/// built-in placeholder when a slot isn't supplied). This minimal port keeps
/// the three closures required and defers those conveniences — every call
/// site is explicit for now.
///
/// # Type parameters
///
/// - `S`: the outer component state the returned [`AnyView`] is generic
///   over (mirrors every other Frust view-producing helper).
/// - `T`: the loaded value type.
/// - `F`: the app's [`Failure`] type.
///
/// # Example
///
/// ```rust,ignore
/// let snapshot = state.messages.get(); // tracked read
/// async_view(
///     snapshot,
///     || any(text("Loading…")),
///     |messages: Vec<Message>| any(Column(
///         messages.into_iter().map(|m| any(text(m.subject))).collect(),
///     )),
///     |failure: AppFailure| any(text(failure.user_message())),
/// )
/// ```
pub fn async_view<S, T, F>(
    state: AsyncState<T, F>,
    loading: impl FnOnce() -> AnyView<S>,
    data: impl FnOnce(T) -> AnyView<S>,
    error: impl FnOnce(F) -> AnyView<S>,
) -> AnyView<S>
where
    S: 'static,
    F: Failure,
{
    match state {
        AsyncState::Loading => loading(),
        AsyncState::Data(value) | AsyncState::Reloading(value) => data(value),
        AsyncState::Error { failure, .. } => error(failure),
    }
}
