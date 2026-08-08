//! Cross-engine conformance suite — the shared behavioral contract every
//! `engine::EngineConn` backend must satisfy, mirroring
//! `frust-shared-preferences`/`frust-secure-storage`'s own
//! `run_conformance_suite` precedent.
//!
//! [`run_conformance_suite`]'s own body and every helper it calls are
//! **engine-agnostic**: they take an `open: &dyn Fn(Target) ->
//! Result<Database, DatabaseError>` closure rather than ever naming an
//! [`Engine`] variant directly, since an `Engine` variant is itself
//! feature-gated (e.g. `Engine::Sqlite` doesn't exist to name at all in a
//! `--no-default-features --features engine-turso` build — see `lib.rs`'s
//! `Engine` doc). Only the per-engine wrapper module below (`
//! sqlite_conformance`) picks a concrete [`Engine`] and hands the suite a
//! closure that captures it — that's also why the suite bodies contain
//! zero `#[cfg(feature = "engine-sqlite")]`: engine-specifics live only in
//! the wrapper mod, so a future turso-backend task can add a sibling
//! `#[cfg(feature = "engine-turso")] mod turso_conformance` (plus its own
//! cross-engine tests) purely additively.
//!
//! Every test opens either an in-memory database ([`Target::Memory`]) or a
//! file under a per-test scratch directory ([`scratch_dir`]) — **never**
//! [`frust_paths::data_dir`] — matching `frust-shared-preferences::file`'s
//! own tempdir-isolation precedent.
//!
//! `open(name)`'s sanitization (rejects an empty name or one containing a
//! path separator) is covered by `lib.rs`'s own `db_file_path_rejects_*`
//! unit tests, not duplicated here — those exercise the same
//! `validate_name`/`db_file_path` logic without ever calling
//! [`frust_paths::data_dir`], so re-testing it through the public
//! `Database::open` here would gain nothing while risking a data-dir touch
//! this module otherwise never makes.
//!
//! **No `CREATE INDEX` in the shared suite**: turso marks indexes
//! experimental/off-by-default, so a `CREATE INDEX` assertion belongs only
//! in `sqlite_conformance`'s own sqlite-only test, not in a helper every
//! engine must pass.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::{self, Target};
use crate::{Database, DatabaseError, Engine, Value};

/// Open a fresh connection for `engine` against `target` — the suite's one
/// way to construct a [`Database`] handle, since it needs explicit engine
/// control (a future turso-backend task targets both `Engine::Sqlite` and
/// `Engine::Turso`) rather than this crate's public default-engine
/// resolution ([`Database::open`]/[`Database::open_at`]).
fn open(engine: Engine, target: Target) -> Result<Database, DatabaseError> {
    engine::open_conn(engine, target).map(Database::from_conn)
}

/// A unique scratch directory under the OS temp dir, namespaced by `tag`
/// (the engine name) and `case` (the test) — never the real data
/// directory. Callers `create_dir_all` it themselves and are expected to
/// best-effort `remove_dir_all` it when done.
fn scratch_dir(tag: &str, case: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "frust-database-conformance-{}-{tag}-{case}-{n}",
        std::process::id()
    ))
}

/// `open(target)`, panicking with `tag` and the underlying error on
/// failure — every conformance assertion after this point assumes a good
/// handle, so a failure here is this test's own setup breaking, not the
/// behavior under test.
fn must_open(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
    target: Target,
) -> Database {
    open(target).unwrap_or_else(|e| panic!("{tag}: failed to open a connection: {e}"))
}

/// [`Database::execute`], panicking with `tag` and `sql` on failure.
fn must_exec(tag: &str, db: &Database, sql: &str, params: impl crate::IntoParams) -> u64 {
    db.execute(sql, params)
        .unwrap_or_else(|e| panic!("{tag}: execute {sql:?} failed: {e}"))
}

/// [`Database::query`], panicking with `tag` and `sql` on failure.
fn must_query(
    tag: &str,
    db: &Database,
    sql: &str,
    params: impl crate::IntoParams,
) -> Vec<crate::Row> {
    db.query(sql, params)
        .unwrap_or_else(|e| panic!("{tag}: query {sql:?} failed: {e}"))
}

/// Run the full suite for one engine, tagged `tag` (used in every failure
/// message so a run against multiple engines — a future turso-backend
/// task's cross-engine tests — always names which engine failed). `open`
/// must open a connection for that same engine against whatever
/// [`Target`] it's given.
pub(crate) fn run_conformance_suite(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    value_round_trip_all_classes(tag, open);
    positional_param_binding(tag, open);
    param_count_mismatch_is_sql_error(tag, open);
    execute_rows_affected(tag, open);
    query_column_names_and_order(tag, open);
    transaction_commits_on_ok(tag, open);
    transaction_rolls_back_on_err(tag, open);
    transaction_no_nesting_surfaces_as_err(tag, open);
    syntax_error_is_sql(tag, open);
    open_at_non_writable_dir_is_storage(tag, open);
    multi_handle_same_file_reads_committed_writes(tag, open);
}

/// CREATE TABLE / INSERT / SELECT round-trip for all five [`Value`]
/// classes, including `i64::MIN`/`MAX`, NaN-free reals, an empty string,
/// an empty blob, and an explicit `NULL`. The probe column is declared
/// `BLOB` (SQLite's "no affinity" declared type) so every class round-trips
/// exactly as given rather than being coerced toward a column affinity.
fn value_round_trip_all_classes(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(
        tag,
        &db,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v BLOB)",
        (),
    );

    let cases: Vec<(&str, Value)> = vec![
        ("null", Value::Null),
        ("integer_zero", Value::Integer(0)),
        ("integer_min", Value::Integer(i64::MIN)),
        ("integer_max", Value::Integer(i64::MAX)),
        ("real", Value::Real(3.5)),
        ("real_negative", Value::Real(-1.25)),
        ("text_empty", Value::Text(String::new())),
        ("text", Value::Text("hello frust".to_string())),
        ("blob_empty", Value::Blob(Vec::new())),
        ("blob", Value::Blob(vec![0u8, 1, 2, 255])),
    ];

    for (label, value) in cases {
        must_exec(tag, &db, "DELETE FROM t", ());
        must_exec(
            tag,
            &db,
            "INSERT INTO t (id, v) VALUES (?1, ?2)",
            [Value::Integer(1), value.clone()],
        );
        let rows = must_query(
            tag,
            &db,
            "SELECT v FROM t WHERE id = ?1",
            [Value::Integer(1)],
        );
        assert_eq!(
            rows.len(),
            1,
            "{tag}/{label}: expected exactly one row back"
        );
        assert_eq!(
            rows[0].get(0),
            Some(&value),
            "{tag}/{label}: round-trip mismatch"
        );
    }
}

/// Positional (`?1`-style) parameter binding maps each parameter to its
/// numbered placeholder, not just "params in statement order happen to
/// land in the first columns" — the two columns are populated from
/// parameters in the order the SQL names them.
fn positional_param_binding(tag: &str, open: &dyn Fn(Target) -> Result<Database, DatabaseError>) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(tag, &db, "CREATE TABLE t (a INTEGER, b TEXT)", ());
    must_exec(
        tag,
        &db,
        "INSERT INTO t (a, b) VALUES (?1, ?2)",
        [Value::Integer(7), Value::Text("seven".to_string())],
    );

    let rows = must_query(tag, &db, "SELECT a, b FROM t", ());
    assert_eq!(rows.len(), 1, "{tag}: expected exactly one row back");
    assert_eq!(rows[0].get(0), Some(&Value::Integer(7)), "{tag}: column a");
    assert_eq!(
        rows[0].get(1),
        Some(&Value::Text("seven".to_string())),
        "{tag}: column b"
    );
}

/// Fewer bound parameters than the statement declares placeholders must
/// surface as [`DatabaseError::Sql`], not panic and not silently bind
/// `NULL` for the missing ones.
fn param_count_mismatch_is_sql_error(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(tag, &db, "CREATE TABLE t (a INTEGER, b INTEGER)", ());

    let result = db.execute("INSERT INTO t (a, b) VALUES (?1, ?2)", [Value::Integer(1)]);
    assert!(
        matches!(result, Err(DatabaseError::Sql { .. })),
        "{tag}: a param-count mismatch must surface as DatabaseError::Sql, got {result:?}"
    );
}

/// `execute`'s rows-affected count: a single-row `INSERT` reports 1, a
/// multi-row `UPDATE`/`DELETE` reports every row it touched.
fn execute_rows_affected(tag: &str, open: &dyn Fn(Target) -> Result<Database, DatabaseError>) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(
        tag,
        &db,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)",
        (),
    );

    let inserted = must_exec(
        tag,
        &db,
        "INSERT INTO t (id, v) VALUES (?1, ?2)",
        [Value::Integer(1), Value::Integer(10)],
    );
    assert_eq!(
        inserted, 1,
        "{tag}: a single INSERT must report 1 row affected"
    );

    must_exec(
        tag,
        &db,
        "INSERT INTO t (id, v) VALUES (?1, ?2)",
        [Value::Integer(2), Value::Integer(10)],
    );
    must_exec(
        tag,
        &db,
        "INSERT INTO t (id, v) VALUES (?1, ?2)",
        [Value::Integer(3), Value::Integer(10)],
    );

    let updated = must_exec(
        tag,
        &db,
        "UPDATE t SET v = ?1 WHERE v = ?2",
        [Value::Integer(20), Value::Integer(10)],
    );
    assert_eq!(
        updated, 3,
        "{tag}: a multi-row UPDATE must report every row it touched"
    );

    let deleted = must_exec(tag, &db, "DELETE FROM t WHERE v = ?1", [Value::Integer(20)]);
    assert_eq!(
        deleted, 3,
        "{tag}: a multi-row DELETE must report every row it touched"
    );
}

/// `query`'s column names are correct and order-stable (reflecting the
/// `SELECT` list's own order, not table-declaration order), and
/// [`crate::Row::get_named`] resolves every one of them.
fn query_column_names_and_order(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(
        tag,
        &db,
        "CREATE TABLE t (id INTEGER, name TEXT, score REAL)",
        (),
    );
    must_exec(
        tag,
        &db,
        "INSERT INTO t (id, name, score) VALUES (?1, ?2, ?3)",
        [
            Value::Integer(1),
            Value::Text("alice".to_string()),
            Value::Real(9.5),
        ],
    );

    let rows = must_query(tag, &db, "SELECT id, name, score FROM t", ());
    assert_eq!(rows.len(), 1, "{tag}: expected exactly one row back");
    let row = &rows[0];
    assert_eq!(row.get(0), Some(&Value::Integer(1)), "{tag}: positional id");
    assert_eq!(
        row.get(1),
        Some(&Value::Text("alice".to_string())),
        "{tag}: positional name"
    );
    assert_eq!(
        row.get(2),
        Some(&Value::Real(9.5)),
        "{tag}: positional score"
    );
    assert_eq!(
        row.get_named("id"),
        Some(&Value::Integer(1)),
        "{tag}: named id"
    );
    assert_eq!(
        row.get_named("name"),
        Some(&Value::Text("alice".to_string())),
        "{tag}: named name"
    );
    assert_eq!(
        row.get_named("score"),
        Some(&Value::Real(9.5)),
        "{tag}: named score"
    );
    assert_eq!(row.get_named("missing"), None, "{tag}: unknown column name");

    // A `SELECT` list order different from the table's declaration order
    // must be reflected as-is, not silently normalized back.
    let reversed = must_query(tag, &db, "SELECT score, name, id FROM t", ());
    assert_eq!(
        reversed[0].get(0),
        Some(&Value::Real(9.5)),
        "{tag}: reversed[0]"
    );
    assert_eq!(
        reversed[0].get(1),
        Some(&Value::Text("alice".to_string())),
        "{tag}: reversed[1]"
    );
    assert_eq!(
        reversed[0].get(2),
        Some(&Value::Integer(1)),
        "{tag}: reversed[2]"
    );
    assert_eq!(
        reversed[0].get_named("score"),
        Some(&Value::Real(9.5)),
        "{tag}: reversed named score"
    );
}

/// A committed transaction's writes are visible after it returns.
fn transaction_commits_on_ok(tag: &str, open: &dyn Fn(Target) -> Result<Database, DatabaseError>) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(tag, &db, "CREATE TABLE t (id INTEGER PRIMARY KEY)", ());

    let result = db.transaction(|txn| {
        txn.execute("INSERT INTO t (id) VALUES (?1)", [Value::Integer(1)])?;
        Ok(())
    });
    assert!(
        result.is_ok(),
        "{tag}: a transaction whose closure returns Ok must itself return Ok, got {result:?}"
    );

    let rows = must_query(tag, &db, "SELECT id FROM t", ());
    assert_eq!(
        rows.len(),
        1,
        "{tag}: a committed transaction's write must be visible after it returns"
    );
}

/// A transaction whose closure returns `Err` rolls back — its writes must
/// not be visible, and the error must propagate as `Err`, not panic.
fn transaction_rolls_back_on_err(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(tag, &db, "CREATE TABLE t (id INTEGER PRIMARY KEY)", ());

    let result: Result<(), DatabaseError> = db.transaction(|txn| {
        txn.execute("INSERT INTO t (id) VALUES (?1)", [Value::Integer(1)])?;
        Err(DatabaseError::Sql {
            message: "forced rollback".to_string(),
        })
    });
    assert!(
        result.is_err(),
        "{tag}: a transaction whose closure returns Err must itself return Err"
    );

    let rows = must_query(tag, &db, "SELECT id FROM t", ());
    assert!(
        rows.is_empty(),
        "{tag}: a rolled-back transaction's write must not be visible"
    );
}

/// v1 has no nested-transaction support: a second `BEGIN` issued through
/// an already-open transaction's own `execute` must surface as
/// [`DatabaseError::Sql`] from the engine — not panic, and not silently
/// succeed — and the outer transaction still rolls back cleanly on that
/// propagated `Err`.
fn transaction_no_nesting_surfaces_as_err(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let db = must_open(tag, open, Target::Memory);
    must_exec(tag, &db, "CREATE TABLE t (id INTEGER PRIMARY KEY)", ());

    let result = db.transaction(|txn| txn.execute("BEGIN", ()));
    assert!(
        matches!(result, Err(DatabaseError::Sql { .. })),
        "{tag}: a nested BEGIN must surface as DatabaseError::Sql, got {result:?}"
    );

    let rows = must_query(tag, &db, "SELECT id FROM t", ());
    assert!(
        rows.is_empty(),
        "{tag}: a failed nested-transaction attempt must leave no rows behind"
    );
}

/// A SQL syntax error surfaces as [`DatabaseError::Sql`] with a non-empty
/// message.
fn syntax_error_is_sql(tag: &str, open: &dyn Fn(Target) -> Result<Database, DatabaseError>) {
    let db = must_open(tag, open, Target::Memory);
    let result = db.execute("THIS IS NOT VALID SQL AT ALL", ());
    match result {
        Err(DatabaseError::Sql { message }) => {
            assert!(!message.is_empty(), "{tag}: Sql error must carry a message");
        }
        other => panic!("{tag}: a syntax error must surface as DatabaseError::Sql, got {other:?}"),
    }
}

/// Opening against a path inside a non-writable directory surfaces
/// [`DatabaseError::Storage`], not [`DatabaseError::Sql`] or a panic.
/// Unix-only (POSIX permission bits); skips itself when the process can
/// write despite the read-only mode bits (e.g. running as `root` in a CI
/// container), rather than false-failing.
#[cfg(unix)]
fn open_at_non_writable_dir_is_storage(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch_dir(tag, "readonly-dir");
    fs::create_dir_all(&dir).expect("create scratch dir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))
        .expect("chmod scratch dir read-only");

    let probe_path = dir.join("probe");
    let can_still_write = fs::File::create(&probe_path).is_ok();
    let _ = fs::remove_file(&probe_path);

    if can_still_write {
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&dir);
        return;
    }

    let result = open(Target::Path(dir.join("blocked.db")));
    match &result {
        Err(DatabaseError::Storage(_)) => {}
        Err(other) => panic!(
            "{tag}: opening in a non-writable directory must surface DatabaseError::Storage, got {other}"
        ),
        Ok(_) => panic!("{tag}: opening in a non-writable directory must fail, but it succeeded"),
    }

    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
    let _ = fs::remove_dir_all(&dir);
}

/// Non-Unix stub: this crate's targets are Unix (Android/iOS/macOS/Linux)
/// plus desktop Windows, and Windows' ACL-based permission model doesn't
/// map onto the same probe — no assertion runs there.
#[cfg(not(unix))]
fn open_at_non_writable_dir_is_storage(
    _tag: &str,
    _open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
}

/// Two [`Database`] handles opened onto the *same* file both see each
/// other's committed writes — every real backend opens in WAL journal
/// mode, which supports concurrent readers alongside one writer (`lib.rs`'s
/// *Threading model* doc).
fn multi_handle_same_file_reads_committed_writes(
    tag: &str,
    open: &dyn Fn(Target) -> Result<Database, DatabaseError>,
) {
    let dir = scratch_dir(tag, "multi-handle");
    fs::create_dir_all(&dir).expect("create scratch dir");
    let path = dir.join("shared.db");

    let writer = must_open(tag, open, Target::Path(path.clone()));
    must_exec(tag, &writer, "CREATE TABLE t (id INTEGER PRIMARY KEY)", ());
    must_exec(
        tag,
        &writer,
        "INSERT INTO t (id) VALUES (?1)",
        [Value::Integer(1)],
    );

    let reader = must_open(tag, open, Target::Path(path));
    let rows = must_query(tag, &reader, "SELECT id FROM t", ());
    assert_eq!(
        rows.len(),
        1,
        "{tag}: a second handle on the same file must see the first handle's committed write"
    );

    must_exec(
        tag,
        &writer,
        "INSERT INTO t (id) VALUES (?1)",
        [Value::Integer(2)],
    );
    let rows_after = must_query(tag, &reader, "SELECT id FROM t", ());
    assert_eq!(
        rows_after.len(),
        2,
        "{tag}: a second handle must see a later committed write too (WAL concurrent readers)"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `engine-sqlite` conformance wrappers — this phase's sole caller of
/// [`run_conformance_suite`]. A future turso-backend task adds a sibling
/// `#[cfg(feature = "engine-turso")] mod turso_conformance` (plus its own
/// cross-engine tests) without needing to touch this module or any suite
/// helper above.
#[cfg(feature = "engine-sqlite")]
mod sqlite_conformance {
    use super::{open, run_conformance_suite};
    use crate::engine::Target;
    use crate::{Database, DatabaseError, Engine, Value};

    fn open_sqlite(target: Target) -> Result<Database, DatabaseError> {
        open(Engine::Sqlite, target)
    }

    #[test]
    fn conformance_suite() {
        run_conformance_suite("sqlite", &open_sqlite);
    }

    /// SQLite-only: `CREATE INDEX` is deliberately kept out of the shared
    /// suite (turso marks indexes experimental/off-by-default — see this
    /// file's module doc), but the sqlite engine itself must still support
    /// it: an index-backed lookup returns the right row.
    #[test]
    fn create_index() {
        let db = open_sqlite(Target::Memory).expect("sqlite: failed to open");
        db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", ())
            .expect("sqlite: CREATE TABLE failed");
        db.execute("CREATE INDEX idx_name ON t (name)", ())
            .expect("sqlite: CREATE INDEX failed");
        db.execute(
            "INSERT INTO t (id, name) VALUES (?1, ?2)",
            [Value::Integer(1), Value::Text("alice".to_string())],
        )
        .expect("sqlite: INSERT failed");

        let rows = db
            .query(
                "SELECT id FROM t WHERE name = ?1",
                [Value::Text("alice".to_string())],
            )
            .expect("sqlite: indexed SELECT failed");
        assert_eq!(
            rows.len(),
            1,
            "sqlite: indexed lookup must return exactly one row"
        );
        assert_eq!(rows[0].get(0), Some(&Value::Integer(1)));
    }
}
