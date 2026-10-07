//! Database section: `frust-database`'s open/create/insert/count smoke —
//! the plugin's whole vertical slice exercised from a real button press, or
//! automatically once when the section is reached through `frustplay://section/db`,
//! and the Android device-gate vehicle for `frust_paths::data_dir()`
//! resolving inside the app process (`Context.getFilesDir()`, no HOME/XDG
//! environment variable the desktop backends fall back to — see
//! `plugins/database/src/lib.rs`'s `Database::open` doc, *Platform
//! locations*).
//!
//! # Smoke sequence
//!
//! "Open + insert" opens (creating if absent)
//! `frust_database::Database::open("playground")`, ensures a `smoke` table
//! exists, inserts one row stamped with the current wall-clock second, and
//! counts every row in the table — the whole sequence run off the UI thread
//! through [`frust::spawn_blocking`] (`plugins/database`'s own module doc,
//! *UI-thread discipline*: every `Database` call blocks synchronously). The
//! result lands three places: this page's own status line, a toast
//! (`state.toasts`, this module's page-fn contract), and a `log::info!`/
//! `log::error!` line carrying [`SMOKE_LOG_PREFIX`] — the on-device gate
//! greps logcat for that exact prefix.
//!
//! "Clear" runs `DELETE FROM smoke` through the same handle-open +
//! `spawn_blocking` seam, so the smoke test can be re-run without
//! accumulating rows across app launches.

use std::time::{SystemTime, UNIX_EPOCH};

use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding,
    RwSignal, Set, SizedBox, Theme, Update, View, any, button, inflexible, row, spawn_blocking,
    spawn_local, text, use_context,
};
use frust_database::{Database, DatabaseError, Value};

use crate::PlaygroundState;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`url_launcher.rs`/`platform_views.rs`/
/// `auth_session.rs`/the demo app's `composite.rs` page each carry the same
/// macro), not shared across files.
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> RwSignal<$ty> {
            thread_local! {
                static SLOT: std::cell::RefCell<Option<RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}

// The last smoke/clear result's rendered status line — empty until the
// first button press.
local_sig!(status_sig, String, String::new());

/// The log line prefix the on-device gate greps logcat for (this module
/// doc's *Smoke sequence* section).
const SMOKE_LOG_PREFIX: &str = "frust-database smoke:";

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context — the same pattern every other section page uses.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// A fixed-width horizontal spacer between the two buttons.
fn gap_h(w: f64) -> FlexChild<PlaygroundState> {
    inflexible(SizedBox(Some(w), None))
}

/// Run the smoke test asynchronously: spawn the blocking operation off the UI
/// thread, log results with [`SMOKE_LOG_PREFIX`], update the status signal, and
/// push a toast message. Called from the "Open + insert" button handler or
/// automatically when the section is reached via a deep link to the DB page.
fn spawn_smoke(status: RwSignal<String>, toasts: RwSignal<Vec<String>>) {
    spawn_local(async move {
        let message = match spawn_blocking(run_smoke).await {
            Ok(Ok(count)) => {
                log::info!("{SMOKE_LOG_PREFIX} ok rows={count}");
                format!("db ok rows={count}")
            }
            Ok(Err(err)) => {
                log::error!("{SMOKE_LOG_PREFIX} error {err}");
                format!("db error: {err}")
            }
            Err(join_err) => {
                log::error!("{SMOKE_LOG_PREFIX} error {join_err}");
                format!("db error: {join_err}")
            }
        };
        status.set(message.clone());
        toasts.update(|queue| queue.push(message));
    });
}

/// Open (creating if absent) `playground`'s database, ensure the `smoke`
/// table exists, insert one row stamped with the current wall-clock second,
/// and return the table's row count.
///
/// A plain blocking function — the caller pairs it with [`spawn_blocking`]
/// so it never runs on the UI thread (`plugins/database`'s own "UI-thread
/// discipline" doc).
fn run_smoke() -> Result<i64, DatabaseError> {
    let db = Database::open("playground")?;
    db.execute(
        "CREATE TABLE IF NOT EXISTS smoke (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL)",
        (),
    )?;
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string();
    db.execute("INSERT INTO smoke (at) VALUES (?1)", [at.as_str()])?;
    let rows = db.query("SELECT COUNT(*) FROM smoke", ())?;
    let count = rows
        .first()
        .and_then(|row| row.get(0))
        .and_then(|value| match value {
            Value::Integer(n) => Some(*n),
            _ => None,
        })
        .unwrap_or(-1);
    Ok(count)
}

/// `DELETE FROM smoke` — same open + `spawn_blocking` seam as [`run_smoke`],
/// so the "Clear" button can reset the smoke test without deleting or
/// migrating the database file itself.
fn run_clear() -> Result<(), DatabaseError> {
    let db = Database::open("playground")?;
    db.execute("DELETE FROM smoke", ())?;
    Ok(())
}

/// See the page-fn contract in [`crate::pages`]. If the DB section is reached
/// via a deep link (`frustplay://section/db`), auto-runs the smoke test once
/// (via [`spawn_smoke`]) before building the page — but **only in a debug
/// build** (`cfg!(debug_assertions)`): an exported/deep-linkable activity
/// must not let an arbitrary external caller make a release build write to
/// its database, so a release build logs and drops the flag instead of
/// running the insert. Reads no [`PlaygroundState`] signal for its own status
/// line (that lives in [`status_sig`]) — only `state.toasts` and
/// `state.auto_run_db_smoke` are accessed; the latter is a plain `Cell`, not a
/// tracked signal, since nothing subscribes to it (see
/// [`PlaygroundState::auto_run_db_smoke`]'s doc comment).
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    // Consume the flag unconditionally (so a release build never leaves it
    // set for a later debug rebuild to auto-run retroactively), then only
    // spawn the smoke test in a debug build.
    if state.auto_run_db_smoke.get() {
        state.auto_run_db_smoke.set(false);
        if cfg!(debug_assertions) {
            spawn_smoke(status_sig(), state.toasts);
        } else {
            log::info!("frust-database smoke: auto-run disabled in release builds");
        }
    }

    let status = status_sig().get();
    let status_line = if status.is_empty() {
        "status: (no attempt yet)".to_string()
    } else {
        format!("status: {status}")
    };

    let children: Vec<AnyView<PlaygroundState>> = vec![
        any(text("Database").size(13.0).color(accent())),
        any(text(
            "frust-database's Database::open/execute/query, exercised from a real button \
                 press \u{2014} the Android device-gate vehicle proving frust_paths::data_dir() \
                 resolves inside the app process (Context.getFilesDir(), no HOME/XDG \
                 environment variable). \u{201c}Open + insert\u{201d} opens \
                 <data_dir>/databases/playground.db, creates the smoke table, inserts a row, \
                 and counts them; \u{201c}Clear\u{201d} empties it for a re-run.",
        )
        .size(11.0)
        .color(muted())),
        any(SizedBox(None, Some(12.0))),
        any(row()
            .child(
                button("Open + insert", |state: &mut PlaygroundState| {
                    spawn_smoke(status_sig(), state.toasts);
                })
                .style(ButtonStyle::Primary)
                .small(),
            )
            .push(gap_h(8.0))
            .child(
                button("Clear", |state: &mut PlaygroundState| {
                    let toasts = state.toasts;
                    spawn_local(async move {
                        let message = match spawn_blocking(run_clear).await {
                            Ok(Ok(())) => "db cleared".to_string(),
                            Ok(Err(err)) => format!("db error: {err}"),
                            Err(join_err) => format!("db error: {join_err}"),
                        };
                        status_sig().set(message.clone());
                        toasts.update(|queue| queue.push(message));
                    });
                })
                .style(ButtonStyle::Secondary)
                .small(),
            )),
        any(SizedBox(None, Some(8.0))),
        any(text(status_line).size(12.0)),
    ];

    Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            children.into_iter().map(inflexible).collect(),
        ),
    )
}
