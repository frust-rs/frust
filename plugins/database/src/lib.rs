//! `frust-database`: a platform-independent, synchronous local SQL API —
//! one public surface over an engine-swappable SQLite-compatible store.
//!
//! # Charter: a pure-Rust plugin, no `frust-plugin`
//!
//! Unlike every other crate under `plugins/` (see `docs/ARCHITECTURE.md`'s
//! Module Structure / `docs/CODE_STANDARDS.md`'s Plugin Conventions), this
//! crate depends on **no** `frust-plugin` — it needs no JNI/platform
//! handle. Both SQL engines it can route to (`rusqlite`'s bundled SQLite
//! today; a future `turso` async engine — see *Engines* below) reach
//! on-disk storage directly through their own FFI/bindings, never through
//! an OS capability API a platform handle would gate. The crate's sole
//! `frust-*` dependency is [`frust_paths`], for [`Database::open`]'s
//! default `<data_dir>/databases/<name>.db` path resolution. An app adds
//! this crate to its own `Cargo.toml` alongside `frust`, the same way
//! every other plugin does; the `frust` facade does not depend on or
//! re-export it.
//!
//! # Engines
//!
//! [`Engine`] names the compiled backend a [`Database`] routes through:
//! [`Engine::Sqlite`] (this crate's default — the `engine-sqlite` feature,
//! `rusqlite`'s bundled, synchronous, in-process SQLite) and `Engine::Turso`
//! (the `engine-turso` feature — an async engine bridged onto this crate's
//! synchronous API, see `turso.rs`; not doc-linkable here since the variant
//! only exists when that feature is compiled). [`OpenOptions::engine`] picks
//! one explicitly;
//! [`Database::open`]/[`Database::open_in_memory`]/[`Database::open_at`]
//! resolve a default instead — Sqlite when compiled, else Turso, else
//! (neither compiled) a [`DatabaseError::Storage`] naming the missing
//! feature, since [`Engine`] itself has no values to report in
//! [`DatabaseError::EngineUnavailable`] once both its variants are
//! compiled out (each is individually feature-gated — see the type's own
//! doc).
//!
//! # UI-thread discipline: pair every call with `spawn_blocking`
//!
//! Every [`Database`] operation is a **blocking** synchronous call — a
//! `rusqlite` statement runs on the calling thread, and the `turso` backend
//! bridges its async engine onto this same blocking call shape (see
//! [`DatabaseError::AsyncContext`]). Like every other plugin's
//! blocking/gated call (`docs/PLUGINS_ARCHITECTURE.md`'s Layer
//! Dependencies), an app must never call [`Database::execute`] /
//! [`Database::query`] / [`Database::transaction`] directly from the UI
//! thread — route it through `frust::spawn_blocking`:
//!
//! ```ignore
//! let db = frust_database::Database::open("app")?;
//! let inserted = frust::spawn_blocking(move || {
//!     db.execute("INSERT INTO notes (body) VALUES (?1)", ["hello"])
//! })
//! .await??;
//! ```
//!
//! Unlike `secure-storage`'s biometric gate or `camera`'s permission/
//! capture calls, this crate has **no code-level UI-thread guard** — there
//! is no shared guard helper in this codebase, and adding one here would
//! need an FFI dependency this pure-Rust crate deliberately carries none
//! of (see *Charter* above). UI-thread discipline is docs-only here,
//! matching `secure-storage`'s own precedent for calls it can't cheaply
//! guard in code.
//!
//! # Threading model: one serialized connection per handle
//!
//! A [`Database`] wraps exactly one engine connection behind a
//! crate-private `Mutex<Box<dyn engine::EngineConn>>`, so every call
//! through one handle is serialized — `Database` is `Send + Sync` and
//! cheap to share (e.g. behind an `Arc`), but two concurrent calls on the
//! *same* handle queue rather than run in parallel. **Open multiple
//! handles for concurrent readers**: every real backend opens its file in
//! WAL journal mode (see *Interop discipline* below), which supports
//! concurrent readers alongside one writer, but only across separate
//! connections — an app that wants read parallelism opens more than one
//! `Database` handle onto the same file rather than sharing one handle
//! across threads expecting internal parallelism.
//!
//! # Interop discipline (enforced by backends, not this module)
//!
//! Every real backend opens its connection in **WAL journal mode**, and
//! neither engine turns on a session-extension-style MVCC layer or
//! on-disk encryption (no `SQLCipher`/equivalent) — a `frust-database`
//! file is a plain, unencrypted WAL SQLite file any standard `sqlite3`
//! tool can open. This crate's public API doesn't enforce that directly;
//! each backend module's own doc comment documents how it configures its
//! connection.
//!
//! # v1 scope
//!
//! Positional parameters only ([`Value`] / [`IntoParams`]); no
//! prepared-statement cache, no streaming cursors, no migrations — a plain
//! `execute`/`query`/`transaction` surface. Future enhancements (not v1):
//! named parameters, a statement cache, streaming query results, and a
//! migration runner.

mod engine;

// The default engine backend — see `sqlite.rs`'s own module doc.
#[cfg(feature = "engine-sqlite")]
mod sqlite;

// The non-default, opt-in engine backend — see `turso.rs`'s own module doc.
#[cfg(feature = "engine-turso")]
mod turso;

// The cross-engine conformance suite — see `conformance.rs`'s own module
// doc. Gated on the `conformance` feature too, not just `test`, so a future
// harness can compile it without a full test build.
#[cfg(any(test, feature = "conformance"))]
pub(crate) mod conformance;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use engine::{EngineConn, Target};

/// The five SQLite storage classes — this crate's currency for both
/// statement parameters and result-row values.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// A signed 64-bit integer.
    Integer(i64),
    /// A 64-bit IEEE-754 float.
    Real(f64),
    /// A UTF-8 string.
    Text(String),
    /// An arbitrary byte string.
    Blob(Vec<u8>),
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Integer(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Integer(i64::from(v))
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Real(v)
    }
}

impl From<bool> for Value {
    /// `false` → `0`, `true` → `1` — SQLite has no native boolean storage
    /// class.
    fn from(v: bool) -> Self {
        Value::Integer(i64::from(v))
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Blob(v)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    /// `None` → [`Value::Null`]; `Some(v)` → `v`'s own conversion.
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

/// One result row: column names paired with their [`Value`]s, in
/// statement column order.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    columns: Vec<String>,
    values: Vec<Value>,
}

impl Row {
    /// Build a row from parallel column-name/value lists — the
    /// constructor a backend module ([`sqlite`], the future `turso`) uses
    /// to report a query result; not part of this crate's public API.
    ///
    /// Unused today: `sqlite.rs`'s stub `query` never actually returns a
    /// row (see its module doc) — the sqlite-backend task's real
    /// implementation is this constructor's first caller.
    #[allow(
        dead_code,
        reason = "first real caller lands with the sqlite-backend task"
    )]
    pub(crate) fn new(columns: Vec<String>, values: Vec<Value>) -> Self {
        Self { columns, values }
    }

    /// The value at positional column `idx`, or `None` if out of range.
    pub fn get(&self, idx: usize) -> Option<&Value> {
        self.values.get(idx)
    }

    /// The value of the column named `name`, or `None` if no column has
    /// that name. Matches the first column with that name if a statement
    /// produced duplicate column names.
    pub fn get_named(&self, name: &str) -> Option<&Value> {
        self.columns
            .iter()
            .position(|c| c == name)
            .and_then(|i| self.values.get(i))
    }
}

mod sealed {
    /// Closes [`super::IntoParams`] to this crate's own conversions — see
    /// that trait's doc.
    pub trait Sealed {}
}

/// Types that can be passed as SQL statement parameters to
/// [`Database::execute`] / [`Database::query`] / [`Transaction::execute`] /
/// [`Transaction::query`].
///
/// Sealed (see the private `sealed::Sealed` supertrait): only this
/// crate's own conversions — `()` (no parameters), and an array or slice
/// of anything [`Into<Value>`] (including [`Value`] itself, via its
/// reflexive `Into`) — implement it. Positional only, matching this
/// crate's v1 scope (module doc).
pub trait IntoParams: sealed::Sealed {
    /// Convert into this crate's positional parameter list.
    fn into_params(self) -> Vec<Value>;
}

impl sealed::Sealed for () {}
impl IntoParams for () {
    fn into_params(self) -> Vec<Value> {
        Vec::new()
    }
}

impl<T: Into<Value> + Clone> sealed::Sealed for &[T] {}
impl<T: Into<Value> + Clone> IntoParams for &[T] {
    fn into_params(self) -> Vec<Value> {
        self.iter().cloned().map(Into::into).collect()
    }
}

impl<T: Into<Value>, const N: usize> sealed::Sealed for [T; N] {}
impl<T: Into<Value>, const N: usize> IntoParams for [T; N] {
    fn into_params(self) -> Vec<Value> {
        self.into_iter().map(Into::into).collect()
    }
}

/// Errors from a [`Database`] operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant rather than only displaying it.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum DatabaseError {
    /// A database path couldn't be resolved, created, or otherwise
    /// accessed — an unset data directory, an invalid database name, or an
    /// underlying filesystem failure.
    #[error("database storage error: {0}")]
    Storage(String),

    /// The SQL engine reported a statement failure (a syntax error, a
    /// constraint violation, a type mismatch — whatever the engine itself
    /// surfaces).
    #[error("SQL error: {message}")]
    Sql {
        /// The engine-reported failure message.
        message: String,
    },

    /// Reserved for async-bridging backends: a synchronous call ran from
    /// inside an async runtime worker thread, which the turso engine's
    /// async bridge can't safely block on. `engine-sqlite`'s connection
    /// never raises this — declared now so this enum stays additive-stable
    /// once the turso backend needs it.
    #[error("a synchronous database call ran from inside an async runtime worker")]
    AsyncContext,

    /// The requested (or resolved-default) [`Engine`] isn't compiled into
    /// this build.
    #[error("engine {0:?} is not available in this build")]
    EngineUnavailable(Engine),
}

/// The compiled SQL engine backend a [`Database`] routes through.
///
/// Each variant is gated on the feature that compiles its backend module,
/// so a build with only one engine feature on can never even *name* the
/// other's variant — see the module doc's *Engines* section for how
/// [`Database`]'s default-engine resolution accounts for this.
/// `#[non_exhaustive]`: a future backend adds a variant without breaking
/// an exhaustive match outside this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Engine {
    /// `rusqlite`'s bundled, synchronous, in-process SQLite — this crate's
    /// default engine.
    #[cfg(feature = "engine-sqlite")]
    Sqlite,
    /// The turso async engine, bridged onto this crate's synchronous API —
    /// see `turso.rs`'s module doc for the bridge design.
    #[cfg(feature = "engine-turso")]
    Turso,
}

/// Builder for [`Database::open_with`].
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    engine: Option<Engine>,
}

impl OpenOptions {
    /// A fresh, default builder — no explicit engine (resolves to this
    /// crate's compiled default at open time).
    pub fn new() -> Self {
        Self::default()
    }

    /// Explicitly select the engine to open with, overriding the compiled
    /// default.
    pub fn engine(mut self, engine: Engine) -> Self {
        self.engine = Some(engine);
        self
    }
}

/// A handle onto one SQL database.
///
/// `Send + Sync`, cheap to share behind an `Arc` — see the module doc's
/// *Threading model* section for what that concurrency contract does and
/// doesn't buy a caller. Every operation is a blocking synchronous call;
/// see the module doc's *UI-thread discipline* section before calling one
/// from the platform UI thread.
pub struct Database {
    conn: Mutex<Box<dyn EngineConn>>,
}

impl Database {
    fn from_conn(conn: Box<dyn EngineConn>) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// Open (creating if absent) the named database at this crate's
    /// standard location, `<data_dir>/databases/<name>.db`
    /// ([`frust_paths::data_dir`]), using the compiled default engine.
    ///
    /// `name` is sanitized: it must be non-empty and contain no path
    /// separator (`/` or `\`) — see [`Self::open_with`] for choosing a
    /// different engine, and [`Self::open_at`] for an explicit path.
    ///
    /// # Errors
    /// [`DatabaseError::Storage`] if `name` is invalid, no data directory
    /// can be resolved (an unset `HOME`/`APPDATA` — this crate never
    /// guesses a fallback that could silently write into the process's
    /// current directory, matching `frust-shared-preferences`'s own
    /// `FileStore::standard` precedent), the `databases` directory can't be
    /// created, or (only once both engine features are compiled out) no
    /// engine is available at all. [`DatabaseError::EngineUnavailable`] if
    /// an explicitly-requested engine isn't compiled in.
    pub fn open(name: &str) -> Result<Self, DatabaseError> {
        Self::open_with(name, OpenOptions::default())
    }

    /// Open a private, non-shared in-memory database using the compiled
    /// default engine — gone once this handle is dropped.
    ///
    /// # Errors
    /// See [`Self::open`].
    pub fn open_in_memory() -> Result<Self, DatabaseError> {
        let engine = default_engine()?;
        let conn = engine::open_conn(engine, Target::Memory)?;
        Ok(Self::from_conn(conn))
    }

    /// Open (creating if absent) the database at an explicit path, using
    /// the compiled default engine — bypasses [`Self::open`]'s standard
    /// location and name sanitization entirely.
    ///
    /// # Errors
    /// See [`Self::open`].
    pub fn open_at(path: &Path) -> Result<Self, DatabaseError> {
        let engine = default_engine()?;
        let conn = engine::open_conn(engine, Target::Path(path.to_path_buf()))?;
        Ok(Self::from_conn(conn))
    }

    /// Open (creating if absent) the named database at this crate's
    /// standard location, with an explicit [`OpenOptions`] (currently:
    /// engine selection).
    ///
    /// # Errors
    /// See [`Self::open`].
    pub fn open_with(name: &str, options: OpenOptions) -> Result<Self, DatabaseError> {
        let path = resolve_db_path(name)?;
        let engine = match options.engine {
            Some(engine) => engine,
            None => default_engine()?,
        };
        let conn = engine::open_conn(engine, Target::Path(path))?;
        Ok(Self::from_conn(conn))
    }

    /// Run a non-row-returning statement (`INSERT`/`UPDATE`/`DELETE`/DDL),
    /// returning the number of rows affected.
    ///
    /// # Errors
    /// [`DatabaseError::Sql`] if the engine rejects the statement.
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64, DatabaseError> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(sql, &params.into_params())
    }

    /// Run a row-returning statement (`SELECT`), returning every resulting
    /// row.
    ///
    /// # Errors
    /// [`DatabaseError::Sql`] if the engine rejects the statement.
    pub fn query(&self, sql: &str, params: impl IntoParams) -> Result<Vec<Row>, DatabaseError> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query(sql, &params.into_params())
    }

    /// Run `f` inside a `BEGIN`/`COMMIT`/`ROLLBACK` transaction — the same
    /// plain SQL on both engines, run through the engine seam. Commits on
    /// `Ok`, rolls back on `Err`, and returns whatever `f` returned (or its
    /// error).
    ///
    /// # Errors
    /// `f`'s own error, if it returns `Err` (after rolling back). A
    /// `BEGIN`/`COMMIT`/`ROLLBACK` statement itself failing also surfaces
    /// as [`DatabaseError::Sql`].
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Transaction) -> Result<T, DatabaseError>,
    ) -> Result<T, DatabaseError> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("BEGIN", &[])?;
        // Scoped so `txn`'s borrow of `conn` ends before `conn` is used
        // again below (COMMIT/ROLLBACK) — `Transaction` holds no resource
        // of its own to release, only this borrow.
        let result = {
            let txn = Transaction {
                conn: RefCell::new(&mut **conn),
            };
            f(&txn)
        };
        match result {
            Ok(value) => {
                conn.execute("COMMIT", &[])?;
                Ok(value)
            }
            Err(err) => {
                // Best-effort: report the original failure even if the
                // rollback statement itself also fails (an already-broken
                // connection shouldn't hide the caller's real error).
                let _ = conn.execute("ROLLBACK", &[]);
                Err(err)
            }
        }
    }
}

/// A handle to one open transaction, passed to [`Database::transaction`]'s
/// closure.
///
/// Borrows the parent [`Database`]'s already-locked connection for the
/// transaction's lifetime — `execute`/`query` take `&self` (via an
/// internal `RefCell`) so the closure can call either any number of times
/// without needing `&mut`.
pub struct Transaction<'a> {
    conn: RefCell<&'a mut dyn EngineConn>,
}

impl Transaction<'_> {
    /// Run a non-row-returning statement inside this transaction. See
    /// [`Database::execute`].
    ///
    /// # Errors
    /// [`DatabaseError::Sql`] if the engine rejects the statement.
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64, DatabaseError> {
        self.conn.borrow_mut().execute(sql, &params.into_params())
    }

    /// Run a row-returning statement inside this transaction. See
    /// [`Database::query`].
    ///
    /// # Errors
    /// [`DatabaseError::Sql`] if the engine rejects the statement.
    pub fn query(&self, sql: &str, params: impl IntoParams) -> Result<Vec<Row>, DatabaseError> {
        self.conn.borrow_mut().query(sql, &params.into_params())
    }
}

// --- Default-engine resolution ---------------------------------------
//
// Exactly one of these three definitions compiles for any feature
// combination (the three `cfg`s are mutually exclusive and exhaustive):
// prefer Sqlite when compiled, else Turso, else — since `Engine` then has
// no values at all to embed in `DatabaseError::EngineUnavailable` — a
// `DatabaseError::Storage` naming the missing feature.

#[cfg(feature = "engine-sqlite")]
fn default_engine() -> Result<Engine, DatabaseError> {
    Ok(Engine::Sqlite)
}

#[cfg(all(not(feature = "engine-sqlite"), feature = "engine-turso"))]
fn default_engine() -> Result<Engine, DatabaseError> {
    Ok(Engine::Turso)
}

#[cfg(not(any(feature = "engine-sqlite", feature = "engine-turso")))]
fn default_engine() -> Result<Engine, DatabaseError> {
    Err(DatabaseError::Storage(
        "no SQL engine compiled into this build — enable the `engine-sqlite` or `engine-turso` \
         feature"
            .into(),
    ))
}

// --- Path resolution ---------------------------------------------------

/// `<base>/databases/<name>.db` — the join `Self::open`'s standard
/// location applies on top of a resolved [`frust_paths::data_dir`].
/// Factored out from [`resolve_db_path`] so it's testable without ever
/// touching a real data directory.
fn db_file_path(base: &Path, name: &str) -> Result<PathBuf, DatabaseError> {
    validate_name(name)?;
    Ok(base.join("databases").join(format!("{name}.db")))
}

/// `name` must be non-empty and contain no path separator (`/` or `\`,
/// checked on every target so behavior doesn't depend on the host OS) — a
/// database name is a bare identifier, never a path fragment.
fn validate_name(name: &str) -> Result<(), DatabaseError> {
    if name.is_empty() {
        return Err(DatabaseError::Storage(
            "database name must not be empty".into(),
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(DatabaseError::Storage(format!(
            "database name {name:?} must not contain a path separator"
        )));
    }
    Ok(())
}

/// [`Database::open`]'s full path resolution: sanitize `name`, resolve the
/// user data directory, join this crate's standard `databases/<name>.db`
/// location, and ensure the `databases` directory exists.
fn resolve_db_path(name: &str) -> Result<PathBuf, DatabaseError> {
    let base = frust_paths::data_dir().ok_or_else(|| {
        DatabaseError::Storage(
            "could not resolve a user data directory (HOME/APPDATA unset)".into(),
        )
    })?;
    let path = db_file_path(&base, name)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| DatabaseError::Storage(format!("creating databases directory: {e}")))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Value conversions -------------------------------------------

    #[test]
    fn value_from_conversions() {
        assert_eq!(Value::from(42i64), Value::Integer(42));
        assert_eq!(Value::from(7i32), Value::Integer(7));
        assert_eq!(Value::from(3.5f64), Value::Real(3.5));
        assert_eq!(Value::from(true), Value::Integer(1));
        assert_eq!(Value::from(false), Value::Integer(0));
        assert_eq!(Value::from("hi"), Value::Text("hi".to_string()));
        assert_eq!(
            Value::from(String::from("hi")),
            Value::Text("hi".to_string())
        );
        assert_eq!(Value::from(vec![1u8, 2, 3]), Value::Blob(vec![1, 2, 3]));
        assert_eq!(Value::from(None::<i64>), Value::Null);
        assert_eq!(Value::from(Some(7i64)), Value::Integer(7));
    }

    // --- Row -----------------------------------------------------------

    #[test]
    fn row_get_and_get_named() {
        let row = Row::new(
            vec!["id".to_string(), "name".to_string()],
            vec![Value::Integer(1), Value::Text("a".to_string())],
        );
        assert_eq!(row.get(0), Some(&Value::Integer(1)));
        assert_eq!(row.get(1), Some(&Value::Text("a".to_string())));
        assert_eq!(row.get(2), None);
        assert_eq!(row.get_named("name"), Some(&Value::Text("a".to_string())));
        assert_eq!(row.get_named("missing"), None);
    }

    // --- IntoParams ------------------------------------------------------

    #[test]
    fn into_params_shapes() {
        assert_eq!(().into_params(), Vec::<Value>::new());
        assert_eq!(
            [1i64, 2i64].into_params(),
            vec![Value::Integer(1), Value::Integer(2)]
        );
        let values = [Value::Text("a".to_string())];
        assert_eq!(values.into_params(), vec![Value::Text("a".to_string())]);
        let slice: &[i64] = &[3, 4];
        assert_eq!(
            slice.into_params(),
            vec![Value::Integer(3), Value::Integer(4)]
        );
    }

    // --- Name sanitization / path shape ---------------------------------

    #[test]
    fn db_file_path_shape() {
        let base = Path::new("/tmp/frust-database-test-base");
        let path = db_file_path(base, "app").unwrap();
        assert_eq!(path, base.join("databases").join("app.db"));
    }

    #[test]
    fn db_file_path_rejects_path_separator() {
        assert!(db_file_path(Path::new("/tmp/x"), "a/b").is_err());
        assert!(db_file_path(Path::new("/tmp/x"), "a\\b").is_err());
    }

    #[test]
    fn db_file_path_rejects_empty_name() {
        assert!(db_file_path(Path::new("/tmp/x"), "").is_err());
    }

    // --- Default-engine resolution ---------------------------------------

    #[test]
    #[cfg(feature = "engine-sqlite")]
    fn default_engine_prefers_sqlite_when_compiled() {
        assert_eq!(default_engine().unwrap(), Engine::Sqlite);
    }

    #[test]
    #[cfg(all(not(feature = "engine-sqlite"), feature = "engine-turso"))]
    fn default_engine_falls_back_to_turso() {
        assert_eq!(default_engine().unwrap(), Engine::Turso);
    }

    #[test]
    #[cfg(not(any(feature = "engine-sqlite", feature = "engine-turso")))]
    fn default_engine_errors_when_nothing_compiled() {
        assert!(default_engine().is_err());
    }
}
