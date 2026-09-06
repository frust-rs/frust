//! `engine-sqlite` backend: `rusqlite`'s bundled, synchronous, in-process
//! SQLite — this crate's default engine (see `lib.rs`'s *Engines* section).
//!
//! # Interop discipline: we set WAL, never `mvcc`/cipher pragmas
//!
//! Every file-backed connection this module opens is switched to **WAL**
//! journal mode immediately after open, before any other statement runs.
//! This is deliberate, not cosmetic: the future `turso` engine cannot write
//! rollback-journal (`DELETE`-mode) files at all, and would *mutate* a
//! non-WAL file to WAL the moment it opened one — so if this backend left a
//! freshly-created file in SQLite's default `DELETE` journal mode, a
//! cross-engine round-trip (open with `engine-sqlite`, later reopen the
//! same file with `engine-turso`) would silently rewrite the file's journal
//! mode out from under the caller. Setting WAL ourselves, up front, keeps
//! both engines' view of the file symmetric from the first write onward.
//!
//! `PRAGMA foreign_keys = ON` is set for the same reason on every
//! connection (file or in-memory): it's a per-connection SQLite setting
//! (never persisted to the file), and both engines accept the same pragma,
//! so setting it here costs nothing and keeps `FOREIGN KEY` constraints
//! enforced consistently regardless of which engine opened the connection.
//!
//! No other engine-specific `PRAGMA` belongs here: never `journal_mode =
//! mvcc`, never a `cipher`/`hexkey` pragma (see `lib.rs`'s module doc,
//! *Interop discipline* section — a `frust-database` file is always a
//! plain, unencrypted, standard-SQLite-tool-readable file).

use rusqlite::types::Value as SqliteValue;
use rusqlite::{Connection, ErrorCode};

use crate::engine::{EngineConn, Target};
use crate::{DatabaseError, Row, Value};

/// A live `rusqlite` connection — see the module doc for the pragmas every
/// instance is opened with.
pub(crate) struct SqliteConn(Connection);

/// Open a connection against `target`, applying this module's interop
/// pragmas (see module doc) before returning.
///
/// # Errors
/// [`DatabaseError::Storage`] if the underlying open itself fails for a
/// path/IO reason (an unwritable directory, a corrupt/non-database file,
/// permission denied — see [`open_err`]); [`DatabaseError::Sql`] if the
/// connection opens but a pragma statement is rejected.
pub(crate) fn open(target: Target) -> Result<SqliteConn, DatabaseError> {
    let (conn, is_file) = match target {
        Target::Path(path) => (Connection::open(&path).map_err(open_err)?, true),
        Target::Memory => (Connection::open_in_memory().map_err(open_err)?, false),
    };

    // File targets only: turso cannot write rollback-journal files and
    // would mutate a non-WAL file to WAL on its own open — see module doc.
    if is_file {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(open_err)?;
    }
    // Every connection, file or in-memory: per-connection, both engines
    // accept it — see module doc.
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(open_err)?;

    Ok(SqliteConn(conn))
}

/// Map a `rusqlite::Error` from an `open`-time call (the connection open
/// itself, or one of this module's post-open pragma statements) to this
/// crate's error type.
///
/// A path/IO-level SQLite error code (can't open the file, isn't a
/// database, I/O failure, permission denied) becomes
/// [`DatabaseError::Storage`], since it names a storage-access problem, not
/// a rejected SQL statement; every other `rusqlite::Error` (e.g. a rejected
/// pragma) falls through to the same [`DatabaseError::Sql`] mapping every
/// other call in this module uses (see [`sql_err`]).
fn open_err(e: rusqlite::Error) -> DatabaseError {
    let is_storage_level = matches!(
        e.sqlite_error_code(),
        Some(
            ErrorCode::CannotOpen
                | ErrorCode::SystemIoFailure
                | ErrorCode::NotADatabase
                | ErrorCode::PermissionDenied
        )
    );
    if is_storage_level {
        DatabaseError::Storage(e.to_string())
    } else {
        sql_err(e)
    }
}

/// Map a `rusqlite::Error` from a statement (`execute`/`query`/prepare) to
/// [`DatabaseError::Sql`], preserving the engine's own message text
/// (`Display`, not `Debug`).
fn sql_err(e: rusqlite::Error) -> DatabaseError {
    DatabaseError::Sql {
        message: e.to_string(),
    }
}

/// [`Value`] → `rusqlite::types::Value`: the five SQLite storage classes
/// are 1:1 between the two enums (see this crate's `Value` doc).
fn to_sqlite_value(v: &Value) -> SqliteValue {
    match v {
        Value::Null => SqliteValue::Null,
        Value::Integer(i) => SqliteValue::Integer(*i),
        Value::Real(f) => SqliteValue::Real(*f),
        Value::Text(s) => SqliteValue::Text(s.clone()),
        Value::Blob(b) => SqliteValue::Blob(b.clone()),
    }
}

/// `rusqlite::types::Value` → [`Value`]: the inverse of [`to_sqlite_value`].
fn from_sqlite_value(v: SqliteValue) -> Value {
    match v {
        SqliteValue::Null => Value::Null,
        SqliteValue::Integer(i) => Value::Integer(i),
        SqliteValue::Real(f) => Value::Real(f),
        SqliteValue::Text(s) => Value::Text(s),
        SqliteValue::Blob(b) => Value::Blob(b),
    }
}

impl EngineConn for SqliteConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, DatabaseError> {
        let bound = rusqlite::params_from_iter(params.iter().map(to_sqlite_value));
        let affected = self.0.execute(sql, bound).map_err(sql_err)?;
        Ok(affected as u64)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, DatabaseError> {
        let mut stmt = self.0.prepare(sql).map_err(sql_err)?;
        // Captured before `query()` re-borrows `stmt` mutably below — see
        // `Statement::column_names`'s own doc for why this crate doesn't
        // defer the read past the first step (a concurrently-altered
        // schema mid-query is out of scope for this crate's v1 API).
        let column_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let bound = rusqlite::params_from_iter(params.iter().map(to_sqlite_value));
        let mut rows = stmt.query(bound).map_err(sql_err)?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(sql_err)? {
            let mut values = Vec::with_capacity(column_names.len());
            for i in 0..column_names.len() {
                let v: SqliteValue = row.get(i).map_err(sql_err)?;
                values.push(from_sqlite_value(v));
            }
            out.push(Row::new(column_names.clone(), values));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> SqliteConn {
        open(Target::Memory).expect("in-memory open should succeed")
    }

    // --- Open: in-memory ---------------------------------------------------

    #[test]
    fn open_in_memory_works() {
        let mut conn = open_memory();
        let affected = conn
            .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", &[])
            .unwrap();
        assert_eq!(affected, 0);
    }

    // --- Open: WAL on file targets ------------------------------------------

    #[test]
    fn file_open_sets_wal_journal_mode() {
        let dir = std::env::temp_dir().join(format!(
            "frust-database-sqlite-wal-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wal.db");

        let mut conn = open(Target::Path(path.clone())).expect("file open should succeed");
        let rows = conn.query("PRAGMA journal_mode", &[]).unwrap();
        assert_eq!(rows.len(), 1);
        let mode = match rows[0].get(0).unwrap() {
            Value::Text(s) => s.to_lowercase(),
            other => panic!("expected a text journal_mode, got {other:?}"),
        };
        assert_eq!(mode, "wal");

        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- execute: rows-affected counts --------------------------------------

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

    // --- query: value round-tripping (backend-local smoke check; the full
    // cross-engine sweep lives in the conformance suite) --------------------

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

    // --- Errors: SQL failure surfaces message text --------------------------

    #[test]
    fn sql_error_surfaces_message_text() {
        let mut conn = open_memory();
        let err = conn.execute("NOT VALID SQL", &[]).unwrap_err();
        match err {
            DatabaseError::Sql { message } => {
                assert!(
                    !message.is_empty() && !message.starts_with("SqliteFailure"),
                    "expected the engine's own Display message, got {message:?}"
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
                assert!(message.contains("no_such_table") || !message.is_empty());
            }
            other => panic!("expected DatabaseError::Sql, got {other:?}"),
        }
    }
}
