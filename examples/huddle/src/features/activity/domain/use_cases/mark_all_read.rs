//! [`MarkAllRead`] — the synchronous, infallible "mark all read" op.
//!
//! [`compose_mark_all_read`] is the pure composition (every row marked
//! read), unit-tested in isolation; [`MarkAllRead`] is the thin `UseCase`
//! wrapper around it — the single-op shape
//! [`crate::features::settings::SetTheme`] establishes: a pure composition
//! fn plus a thin `UseCase` wrapper. Never fails; reuses [`HuddleFailure`]
//! purely to satisfy the trait bound.

use clean_signals::UseCase;

use crate::failure::HuddleFailure;
use crate::features::activity::domain::models::ActivityRow;

/// Pure composition: every row marked read.
pub fn compose_mark_all_read(rows: Vec<ActivityRow>) -> Vec<ActivityRow> {
    rows.into_iter()
        .map(|row| ActivityRow {
            unread: false,
            ..row
        })
        .collect()
}

/// The synchronous "mark all read" op (top-bar action) — the single place the
/// feed's unread flags are cleared.
pub struct MarkAllRead;

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl UseCase for MarkAllRead {
    type Params = Vec<ActivityRow>;
    type Output = Vec<ActivityRow>;
    type Failure = HuddleFailure;

    async fn execute(&self, rows: Vec<ActivityRow>) -> Result<Vec<ActivityRow>, HuddleFailure> {
        Ok(compose_mark_all_read(rows))
    }
}
