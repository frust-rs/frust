//! `engine-turso` backend: the Turso engine (a pure-Rust, SQLite-compatible
//! rewrite) bridged from its `async` API onto this crate's synchronous engine
//! seam — non-default, opt-in per `lib.rs`'s *Engines* section.
//!
//! # The bridge: one crate-owned runtime on its own thread
//!
//! Turso's whole API is `async`, and this crate's seam
//! ([`crate::engine::EngineConn`]) is synchronous, so something has to block.
//! This module owns a single process-wide **bridge thread** running a
//! current-thread tokio runtime (`rt` feature only — this crate never spawns a
//! worker pool, a timer, or a socket of its own; `frust-reactive` owns the
//! process's real multi-thread runtime, see `docs/CORE_ARCHITECTURE.md`).
//! Every seam call builds its future, hands it to that runtime, and parks on a
//! channel until the result comes back.
//!
//! The future is driven on the bridge thread rather than on the caller's
//! thread *because the caller's thread may be a runtime worker*:
//! `Runtime::block_on` panics ("Cannot start a runtime from within a runtime")
//! when the calling thread is already driving one, and a panic is not an
//! outcome this seam may produce — release builds are `panic = "abort"`
//! (`docs/DEVELOPMENT.md`'s release-profile hardening), so it would take the
//! process down. Off-thread execution also means a call from an async context
//! can never deadlock: the bridge runtime is independent of the caller's, so
//! the work always completes and the (mis-parked) caller always wakes.
//!
//! **Every handle shares this one bridge — additional handles do not buy
//! read parallelism.** `lib.rs`'s *Threading model* section documents
//! `engine-sqlite`'s "open multiple handles for concurrent readers"
//! guarantee; it does not hold here. However many [`TursoConn`]s an app
//! opens — whether through one or many [`crate::Database`] handles onto the
//! same file — each seam call still hands its future to this single
//! current-thread runtime and parks until it returns, so their operations
//! serialize/interleave on the one bridge thread rather than run in
//! wall-clock parallel. What multiple handles still buy is correctness:
//! each is an independent connection with its own committed-write
//! visibility, not a faster read path.
//!
//! # Why the [`DatabaseError::AsyncContext`] guard is a *provable subset*
//!
//! Blocking a runtime worker is still a contract violation even when it can't
//! panic, and this seam reports [`DatabaseError::AsyncContext`] where it can
//! *prove* one. Proving it is the hard part: tokio publishes no predicate for
//! "this thread is currently driving the runtime". Measured on tokio 1.53
//! (`Handle::try_current()` / `task::try_id()` / whether `block_on` would
//! panic):
//!
//! | Calling context | `try_current` | `try_id` | `block_on` |
//! |---|---|---|---|
//! | plain thread | `Err` | `None` | ok |
//! | runtime's own `block_on` body | `Ok` | `None` | **panics** |
//! | spawned async task | `Ok` | `Some` | **panics** |
//! | `spawn_blocking` closure | `Ok` | `Some` | ok |
//!
//! A blocking-pool thread enters the runtime *handle* for its whole lifetime,
//! so `Handle::try_current().is_ok()` alone cannot be the guard: it would
//! reject `frust_reactive::spawn_blocking`, which is exactly the call path
//! every plugin's blocking work is supposed to take (`lib.rs`'s *UI-thread
//! discipline* section, `docs/PLUGINS_ARCHITECTURE.md`). The last two rows are
//! indistinguishable through any public API — same handle, same task id, same
//! thread name — so [`in_async_context`] reports only the case it can prove
//! (row 2: inside a runtime, not inside a task) and leaves the spawned-task
//! case to the same docs-only discipline `lib.rs` already applies to the UI
//! thread. `async_context_is_reported_from_a_runtime_block_on` and
//! `spawn_blocking_is_not_treated_as_an_async_context` are the tripwires if a
//! future tokio release moves either signal.
//!
//! # Interop discipline: WAL asserted, nothing else issued
//!
//! Turso opens a database file in WAL journal mode and cannot write a
//! rollback-journal file at all, so this module does not *set* a journal mode —
//! it reads `PRAGMA journal_mode` back on every file open and refuses a
//! connection that reports anything else, which keeps the promise `sqlite.rs`
//! makes from its side (both engines see the same file the same way). No other
//! engine-specific pragma is issued: never `journal_mode = mvcc`, never a
//! `cipher`/`hexkey` pragma (`lib.rs`'s *Interop discipline* section — a
//! `frust-database` file is a plain, unencrypted, standard-SQLite-readable
//! file). `PRAGMA foreign_keys = ON` is issued on every connection for parity
//! with `sqlite.rs`, which documents the same per-connection setting.

use std::future::Future;
use std::sync::OnceLock;
use std::sync::mpsc;

use tokio::runtime::Handle;

use crate::engine::{EngineConn, Target};
use crate::{DatabaseError, Row, Value};

// --- The bridge ---------------------------------------------------------

/// The bridge thread's runtime handle, started on first use.
///
/// The `Result` is cached (not retried): a runtime that couldn't be built once
/// won't build on the next call either, and retrying per call would spawn a
/// thread per failed attempt. The error is kept as a string because the
/// underlying `io::Error` isn't `Clone` and this value is handed out by
/// reference forever.
fn bridge() -> Result<&'static Handle, DatabaseError> {
    static BRIDGE: OnceLock<Result<Handle, String>> = OnceLock::new();
    match BRIDGE.get_or_init(start_bridge) {
        Ok(handle) => Ok(handle),
        Err(message) => Err(DatabaseError::Storage(message.clone())),
    }
}

/// Spawn the bridge thread and wait for the runtime it builds there.
///
/// The thread parks on a never-completing future for the rest of the process's
/// life: the runtime exists purely to drive futures other threads spawn onto
/// it, so there is no "done" for it to reach. It holds no database state of its
/// own — a dropped [`TursoConn`] releases its connection wherever it happens to
/// be dropped.
fn start_bridge() -> Result<Handle, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        // Kept under 15 bytes: longer names are rejected outright by
        // `pthread_setname_np` on Linux/Android.
        .name("frust-turso-db".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = tx.send(Err(format!(
                        "could not start the turso bridge runtime: {e}"
                    )));
                    return;
                }
            };
            let _ = tx.send(Ok(runtime.handle().clone()));
            runtime.block_on(std::future::pending::<()>());
        })
        .map_err(|e| format!("could not start the turso bridge thread: {e}"))?;
    rx.recv()
        .map_err(|_| "the turso bridge thread stopped before reporting its runtime".to_string())?
}

/// True when the calling thread is provably *driving* a tokio runtime, i.e.
/// inside a runtime but not inside a task — see this module's doc for why this
/// is a provable subset rather than a complete test, and why the obvious
/// `Handle::try_current().is_ok()` is the wrong guard.
fn in_async_context() -> bool {
    Handle::try_current().is_ok() && tokio::task::try_id().is_none()
}

/// Run `fut` on the bridge runtime, blocking the calling thread until it
/// finishes — the one place every seam call crosses the async boundary.
///
/// # Errors
/// [`DatabaseError::AsyncContext`] if the caller is provably inside an async
/// runtime ([`in_async_context`]); [`DatabaseError::Storage`] if the bridge
/// thread can't be started or stopped before answering.
fn run<T>(fut: impl Future<Output = T> + Send + 'static) -> Result<T, DatabaseError>
where
    T: Send + 'static,
{
    if in_async_context() {
        return Err(DatabaseError::AsyncContext);
    }
    let handle = bridge()?;
    let (tx, rx) = mpsc::channel();
    handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv().map_err(|_| {
        DatabaseError::Storage(
            "the turso bridge dropped a database call without producing a result".to_string(),
        )
    })
}

// --- Errors -------------------------------------------------------------

/// Map a `turso::Error` from an `open`-time call (the open itself, or one of
/// this module's post-open pragma statements) to this crate's error type.
///
/// Mirrors `sqlite.rs`'s own split, variant for variant: a storage-access
/// failure becomes [`DatabaseError::Storage`], since it names a storage
/// problem rather than a rejected SQL statement; everything else falls through
/// to the same [`DatabaseError::Sql`] mapping every other call here uses.
/// Turso reports both an unreachable path (missing directory) and an
/// unwritable one as `IoError(NotFound | PermissionDenied, "open")` — verified
/// against 0.7.2, since the mapping is what the cross-engine conformance
/// suite's open-failure case rests on.
fn open_err(e: turso::Error) -> DatabaseError {
    let is_storage_level = matches!(
        e,
        turso::Error::IoError(..) | turso::Error::NotAdb(_) | turso::Error::Corrupt(_)
    );
    if is_storage_level {
        DatabaseError::Storage(e.to_string())
    } else {
        sql_err(e)
    }
}

/// Map a `turso::Error` from a statement to [`DatabaseError::Sql`], preserving
/// the engine's own message text (`Display`, not `Debug`).
fn sql_err(e: turso::Error) -> DatabaseError {
    DatabaseError::Sql {
        message: e.to_string(),
    }
}

// --- Value mapping ------------------------------------------------------

/// [`Value`] → `turso::Value`: the five SQLite storage classes are 1:1 between
/// the two enums (see this crate's `Value` doc).
fn to_turso_value(v: &Value) -> turso::Value {
    match v {
        Value::Null => turso::Value::Null,
        Value::Integer(i) => turso::Value::Integer(*i),
        Value::Real(f) => turso::Value::Real(*f),
        Value::Text(s) => turso::Value::Text(s.clone()),
        Value::Blob(b) => turso::Value::Blob(b.clone()),
    }
}

/// `turso::Value` → [`Value`]: the inverse of [`to_turso_value`].
fn from_turso_value(v: turso::Value) -> Value {
    match v {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(i) => Value::Integer(i),
        turso::Value::Real(f) => Value::Real(f),
        turso::Value::Text(s) => Value::Text(s),
        turso::Value::Blob(b) => Value::Blob(b),
    }
}

/// The positional parameter list turso binds from — a plain `Vec` of its own
/// values, which is turso's public `IntoParams` shape for positional binding.
fn to_turso_params(params: &[Value]) -> Vec<turso::Value> {
    params.iter().map(to_turso_value).collect()
}

// --- Connection ---------------------------------------------------------

/// A live turso connection. Every operation on it runs on the bridge thread
/// (see this module's doc); the handle itself is `Send`, as
/// [`EngineConn`] requires.
pub(crate) struct TursoConn {
    conn: turso::Connection,
}

/// Open a connection against `target`, asserting this module's interop
/// invariants (see module doc) before returning.
///
/// # Errors
/// [`DatabaseError::Storage`] if the path isn't valid UTF-8 (turso's builder
/// takes a `&str`), if the open itself fails for a path/IO reason, or if a file
/// target doesn't report WAL journal mode; [`DatabaseError::Sql`] if a pragma
/// statement is rejected; [`DatabaseError::AsyncContext`] if called from a
/// provably async context.
pub(crate) fn open(target: Target) -> Result<TursoConn, DatabaseError> {
    let (path, is_file) = match &target {
        // Turso's own spelling for a private, non-shared in-memory database.
        Target::Memory => (":memory:".to_string(), false),
        Target::Path(path) => {
            let path = path.to_str().ok_or_else(|| {
                DatabaseError::Storage(format!("database path {path:?} is not valid UTF-8"))
            })?;
            (path.to_string(), true)
        }
    };

    let conn = run(async move {
        let db = turso::Builder::new_local(&path).build().await?;
        db.connect()
    })?
    .map_err(open_err)?;

    let mut conn = TursoConn { conn };

    if is_file {
        assert_wal(&mut conn)?;
    }
    // Every connection, file or in-memory: per-connection, both engines accept
    // it — see `sqlite.rs`'s module doc. Issued as a query, not an execute:
    // turso's `execute` refuses a statement that yields a row, and a pragma may.
    conn.query("PRAGMA foreign_keys = ON", &[])?;
    Ok(conn)
}

/// Read `PRAGMA journal_mode` back and refuse anything but WAL.
///
/// Turso opens in WAL and cannot write a rollback journal, so a different mode
/// here means the file is not what either engine expects — reporting it is
/// better than writing into it (see module doc's *Interop discipline*).
fn assert_wal(conn: &mut TursoConn) -> Result<(), DatabaseError> {
    let rows = conn.query("PRAGMA journal_mode", &[])?;
    let mode = match rows.first().and_then(|row| row.get(0)) {
        Some(Value::Text(mode)) => mode.to_lowercase(),
        other => {
            return Err(DatabaseError::Storage(format!(
                "turso reported an unreadable journal_mode: {other:?}"
            )));
        }
    };
    if mode == "wal" {
        Ok(())
    } else {
        Err(DatabaseError::Storage(format!(
            "database file is in {mode} journal mode; frust-database requires WAL"
        )))
    }
}

impl EngineConn for TursoConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, DatabaseError> {
        let conn = self.conn.clone();
        let sql = sql.to_string();
        let params = to_turso_params(params);
        run(async move { conn.execute(sql, params).await })?.map_err(sql_err)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, DatabaseError> {
        let conn = self.conn.clone();
        let sql = sql.to_string();
        let params = to_turso_params(params);
        run(async move {
            let mut rows = conn.query(sql, params).await?;
            // Captured once, before the first step: every row of one statement
            // reports the same columns, and this crate's v1 API has no
            // mid-query schema-change story (same reasoning as `sqlite.rs`).
            let columns = rows.column_names();
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                let mut values = Vec::with_capacity(columns.len());
                for i in 0..columns.len() {
                    values.push(from_turso_value(row.get_value(i)?));
                }
                out.push(Row::new(columns.clone(), values));
            }
            Ok::<_, turso::Error>(out)
        })?
        .map_err(sql_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> TursoConn {
        open(Target::Memory).expect("in-memory open should succeed")
    }

    fn scratch_dir(case: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "frust-database-turso-{case}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- Open: in-memory and file ---------------------------------------

    #[test]
    fn open_in_memory_works() {
        let mut conn = open_memory();
        let affected = conn
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", &[])
            .unwrap();
        assert_eq!(affected, 0);
    }

    #[test]
    fn file_open_round_trips_and_reports_wal() {
        let dir = scratch_dir("file-open");
        let path = dir.join("wal.db");

        let mut conn = open(Target::Path(path)).expect("file open should succeed");
        conn.execute("CREATE TABLE t (name TEXT)", &[]).unwrap();
        conn.execute(
            "INSERT INTO t (name) VALUES (?1)",
            &[Value::Text("a".to_string())],
        )
        .unwrap();

        // `open` already refuses a non-WAL file; assert the reported mode here
        // too so a turso default change fails loudly rather than silently
        // changing which files this engine will touch.
        let rows = conn.query("PRAGMA journal_mode", &[]).unwrap();
        let mode = match rows[0].get(0).unwrap() {
            Value::Text(s) => s.to_lowercase(),
            other => panic!("expected a text journal_mode, got {other:?}"),
        };
        assert_eq!(mode, "wal");

        let rows = conn.query("SELECT name FROM t", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get(0), Some(&Value::Text("a".to_string())));

        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_in_missing_directory_is_storage() {
        let dir = scratch_dir("missing-dir");
        let path = dir.join("no-such-dir").join("x.db");
        match open(Target::Path(path)) {
            Err(DatabaseError::Storage(_)) => {}
            Err(other) => panic!("expected DatabaseError::Storage, got {other:?}"),
            Ok(_) => panic!("opening under a missing directory should fail"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- execute: rows-affected counts ----------------------------------

    #[test]
    fn execute_reports_rows_affected() {
        let mut conn = open_memory();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", &[])
            .unwrap();

        let inserted = conn
            .execute(
                "INSERT INTO t (name) VALUES (?1), (?2), (?3)",
                &[
                    Value::Text("a".to_string()),
                    Value::Text("b".to_string()),
                    Value::Text("c".to_string()),
                ],
            )
            .unwrap();
        assert_eq!(inserted, 3);

        let updated = conn
            .execute(
                "UPDATE t SET name = ?1 WHERE name = ?2",
                &[Value::Text("z".to_string()), Value::Text("a".to_string())],
            )
            .unwrap();
        assert_eq!(updated, 1);

        let deleted = conn.execute("DELETE FROM t", &[]).unwrap();
        assert_eq!(deleted, 3);
    }

    // --- The bridge under contention -------------------------------------

    #[test]
    fn concurrent_callers_share_one_bridge() {
        // Two threads driving two independent connections through the one
        // process-wide bridge runtime: each call must complete rather than
        // wedge behind the other's turn on that single-threaded runtime.
        //
        // This is a correctness/liveness test, not a parallelism test: it
        // asserts every caller eventually finishes and sees its own data,
        // never that any two calls actually ran concurrently (they don't —
        // see this module's doc, "Every handle shares this one bridge").
        let threads: Vec<_> = (0..4)
            .map(|i| {
                std::thread::spawn(move || {
                    let mut conn = open_memory();
                    conn.execute("CREATE TABLE t (v INTEGER)", &[]).unwrap();
                    conn.execute("INSERT INTO t (v) VALUES (?1)", &[Value::Integer(i)])
                        .unwrap();
                    let rows = conn.query("SELECT v FROM t", &[]).unwrap();
                    assert_eq!(rows[0].get(0), Some(&Value::Integer(i)));
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("every bridged caller should finish");
        }
    }

    // --- Transactions: the same plain SQL `lib.rs` drives through the seam --

    #[test]
    fn begin_commit_and_rollback_run_through_the_seam() {
        let mut conn = open_memory();
        conn.execute("CREATE TABLE t (name TEXT)", &[]).unwrap();

        conn.execute("BEGIN", &[]).unwrap();
        conn.execute(
            "INSERT INTO t (name) VALUES (?1)",
            &[Value::Text("kept".to_string())],
        )
        .unwrap();
        conn.execute("COMMIT", &[]).unwrap();

        conn.execute("BEGIN", &[]).unwrap();
        conn.execute(
            "INSERT INTO t (name) VALUES (?1)",
            &[Value::Text("dropped".to_string())],
        )
        .unwrap();
        conn.execute("ROLLBACK", &[]).unwrap();

        let rows = conn.query("SELECT name FROM t", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get(0), Some(&Value::Text("kept".to_string())));
    }

    // --- query: value-class mapping -------------------------------------

    #[test]
    fn query_round_trips_every_value_variant() {
        let mut conn = open_memory();
        conn.execute(
            "CREATE TABLE t (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT)",
            &[],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO t (i, r, t, b, n) VALUES (?1, ?2, ?3, ?4, ?5)",
            &[
                Value::Integer(42),
                Value::Real(3.5),
                Value::Text("hi".to_string()),
                Value::Blob(vec![1, 2, 3]),
                Value::Null,
            ],
        )
        .unwrap();

        let rows = conn.query("SELECT i, r, t, b, n FROM t", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.get(0), Some(&Value::Integer(42)));
        assert_eq!(row.get(1), Some(&Value::Real(3.5)));
        assert_eq!(row.get(2), Some(&Value::Text("hi".to_string())));
        assert_eq!(row.get(3), Some(&Value::Blob(vec![1, 2, 3])));
        assert_eq!(row.get(4), Some(&Value::Null));
        assert_eq!(row.get_named("t"), Some(&Value::Text("hi".to_string())));
    }

    #[test]
    fn integer_bounds_and_empty_blob_round_trip() {
        let mut conn = open_memory();
        conn.execute("CREATE TABLE t (v)", &[]).unwrap();
        for value in [
            Value::Integer(i64::MIN),
            Value::Integer(i64::MAX),
            Value::Blob(Vec::new()),
            Value::Blob(vec![0, 255, 128]),
            Value::Text(String::new()),
            Value::Real(f64::MIN_POSITIVE),
        ] {
            conn.execute("DELETE FROM t", &[]).unwrap();
            conn.execute(
                "INSERT INTO t (v) VALUES (?1)",
                std::slice::from_ref(&value),
            )
            .unwrap();
            let rows = conn.query("SELECT v FROM t", &[]).unwrap();
            assert_eq!(rows[0].get(0), Some(&value));
        }
    }

    // --- Errors ----------------------------------------------------------

    #[test]
    fn sql_error_surfaces_message_text() {
        let mut conn = open_memory();
        let err = conn.execute("NOT VALID SQL", &[]).unwrap_err();
        match err {
            DatabaseError::Sql { message } => {
                assert!(
                    !message.is_empty(),
                    "expected the engine's own message, got an empty string"
                );
            }
            other => panic!("expected DatabaseError::Sql, got {other:?}"),
        }
    }

    #[test]
    fn query_error_surfaces_message_text() {
        let mut conn = open_memory();
        let err = conn.query("SELECT * FROM no_such_table", &[]).unwrap_err();
        match err {
            DatabaseError::Sql { message } => {
                assert!(
                    message.contains("no_such_table"),
                    "expected the table name in {message:?}"
                );
            }
            other => panic!("expected DatabaseError::Sql, got {other:?}"),
        }
    }

    // --- The AsyncContext guard ------------------------------------------

    #[test]
    fn async_context_is_reported_from_a_runtime_block_on() {
        let mut conn = open_memory();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = runtime.block_on(async { conn.execute("SELECT 1", &[]).unwrap_err() });
        assert!(
            matches!(err, DatabaseError::AsyncContext),
            "expected DatabaseError::AsyncContext, got {err:?}"
        );
    }

    #[test]
    fn spawn_blocking_is_not_treated_as_an_async_context() {
        // The sanctioned caller path (`frust_reactive::spawn_blocking`): a
        // blocking-pool thread carries the runtime's handle, so a
        // `Handle::try_current()`-only guard would wrongly reject it — see this
        // module's doc.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let rows = runtime.block_on(async {
            tokio::task::spawn_blocking(|| {
                let mut conn = open_memory();
                conn.query("SELECT 1 AS one", &[])
            })
            .await
            .unwrap()
        });
        let rows = rows.expect("a spawn_blocking caller must not be refused");
        assert_eq!(rows[0].get(0), Some(&Value::Integer(1)));
    }

    #[test]
    fn open_reports_async_context_too() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            match open(Target::Memory) {
                Err(DatabaseError::AsyncContext) => {}
                Err(other) => panic!("expected DatabaseError::AsyncContext, got {other:?}"),
                Ok(_) => panic!("an async-context open should be refused"),
            }
        });
    }
}
