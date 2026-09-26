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
//! *same* handle queue rather than run in parallel. Every real backend
//! opens its file in WAL journal mode (see *Interop discipline* below),
//! which supports concurrent readers alongside one writer, but only
//! across separate connections — an app that wants read parallelism opens
//! more than one `Database` handle onto the same file rather than sharing
//! one handle across threads expecting internal parallelism.
//!
//! **"Open multiple handles for concurrent readers" is an `engine-sqlite`
//! guarantee, not a cross-engine one.** `rusqlite`'s bundled SQLite really
//! does run separate connections' reads on separate OS threads, in
//! parallel. `engine-turso` cannot: every `Database` handle, however many
//! an app opens onto the same file, routes its calls through one
//! process-wide, single-threaded bridge (see `turso.rs`'s module doc, *The
//! bridge*). Additional turso handles still buy correctness and
//! cross-handle write visibility — each is its own connection, seeing the
//! others' commits — but not wall-clock parallelism: their calls
//! serialize/interleave on that one bridge thread exactly as if issued
//! through a single handle.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

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
    ///
    /// Reserved, and unconstructable by any of today's code paths: an
    /// [`Engine`] variant only exists at all when the feature compiling its
    /// backend is on, so an engine a caller can *name* is by construction
    /// compiled in — see [`Database::open`]'s own *Errors* section for what
    /// the neither-engine-compiled build reports instead.
    #[error("engine {0:?} is not available in this build")]
    EngineUnavailable(Engine),

    /// A call re-entered a [`Database`] handle the calling thread already
    /// holds — typically an `execute`/`query`/`transaction` issued on a
    /// captured (e.g. `Arc`-shared) handle from *inside* that same handle's
    /// [`Database::transaction`] closure.
    ///
    /// Reported rather than deadlocking on the handle's non-reentrant
    /// connection mutex: run statements inside a transaction through the
    /// [`Transaction`] handle the closure is given. Calls from *other*
    /// threads are unaffected — they queue on the mutex as the module doc's
    /// *Threading model* section promises.
    #[error("a database call re-entered a handle already locked by the calling thread")]
    Reentrant,
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
    /// The [`thread_token`] of whichever thread currently holds `conn`, or
    /// `0` when it's unheld — the whole state behind [`Self::lock_conn`]'s
    /// reentrancy check. Written only while the lock is held.
    holder: AtomicU64,
}

impl Database {
    fn from_conn(conn: Box<dyn EngineConn>) -> Self {
        Self {
            conn: Mutex::new(conn),
            holder: AtomicU64::new(UNHELD),
        }
    }

    /// Lock this handle's connection, reporting [`DatabaseError::Reentrant`]
    /// instead of deadlocking when the calling thread already holds it.
    ///
    /// The connection sits behind a plain, non-reentrant `std::sync::Mutex`,
    /// so a closure that captured the same handle (an `Arc<Database>` — the
    /// sharing shape this crate recommends) and called back into
    /// `execute`/`query`/`transaction` would otherwise park forever on a
    /// lock only it can release. A blind `try_lock` would be the wrong
    /// detector: it would also refuse legitimate *cross-thread* contention,
    /// which the module doc's *Threading model* section promises will queue.
    /// So the check is by owner identity instead — `holder` carries the
    /// token of the thread holding the lock, written only under the lock, so
    /// a caller that finds its own token there is provably re-entering while
    /// every other caller blocks exactly as before.
    ///
    /// # Poison policy
    /// A poisoned mutex is recovered (`into_inner()`) rather than
    /// propagated, and that is sound **only because of [`RollbackGuard`]**:
    /// the only way a panic can escape *with a transaction open* is a
    /// caller's transaction closure panicking (bare `execute`/`query` can
    /// also panic while holding the lock, but no `BEGIN` ran there, so
    /// nothing is stranded), and that guard's `Drop` issues the `ROLLBACK`
    /// on the way out. So — **assuming that best-effort `ROLLBACK` itself
    /// succeeds** — the connection a later caller recovers here is not left
    /// mid-transaction. If the `ROLLBACK` statement itself fails (I/O error
    /// mid-rollback, disk full), the transaction can remain open with no
    /// taint recorded — an accepted, narrow residual documented in
    /// `docs/LIMITATIONS.md` (`db-rollback-failure-residual`). Were the
    /// guard removed entirely, recovering a poisoned lock would hand out a
    /// connection with an orphaned transaction still open — a silent
    /// data-loss path, not a mere lost error.
    ///
    /// # Errors
    /// [`DatabaseError::Reentrant`] if the calling thread already holds this
    /// handle's connection.
    fn lock_conn(&self) -> Result<ConnGuard<'_>, DatabaseError> {
        let me = thread_token();
        if self.holder.load(Ordering::Acquire) == me {
            return Err(DatabaseError::Reentrant);
        }
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        self.holder.store(me, Ordering::Release);
        Ok(ConnGuard {
            conn,
            holder: &self.holder,
        })
    }

    /// Open (creating if absent) the named database at this crate's
    /// standard location, `<data_dir>/databases/<name>.db`
    /// ([`frust_paths::data_dir`]), using the compiled default engine.
    ///
    /// `name` is sanitized: it must be non-empty and contain no path
    /// separator (`/` or `\`) — see [`Self::open_with`] for choosing a
    /// different engine, and [`Self::open_at`] for an explicit path.
    ///
    /// The `databases/` directory is deliberately shared by every frust app
    /// on the machine (no per-binary `app_stem` component), which is also
    /// why this crate does its own **file-level** read-through on macOS: if
    /// `<data_dir>/databases/<name>.db` doesn't exist but the same shape
    /// under [`frust_paths::legacy_data_dir`] does, the legacy file is
    /// opened where it already lives — nothing is migrated, copied, or
    /// deleted. `data_dir()`'s own built-in macOS fallback probes
    /// `<base>/<app_stem>` and so can never see this crate's paths.
    ///
    /// Platform locations:
    /// - **Android**: `<Context.getFilesDir()>/databases/<name>.db`. This directory
    ///   is installed by the platform shell at `nativeInitPlatform` time; `Database::open`
    ///   must run *after* this initialization completes (typically from app code, not from
    ///   a static initializer).
    /// - **iOS, macOS, Linux, Windows**: `<data_dir>/databases/<name>.db`, where `data_dir`
    ///   is resolved from environment variables (`HOME`, `XDG_DATA_HOME`, `APPDATA`, etc.).
    ///
    /// # Errors
    /// [`DatabaseError::Storage`] if `name` is invalid, no data directory
    /// can be resolved (an unset `HOME`/`APPDATA` — this crate never
    /// guesses a fallback that could silently write into the process's
    /// current directory, matching `frust-shared-preferences`'s own
    /// `FileStore::standard` precedent), the `databases` directory can't be
    /// created, or (only once both engine features are compiled out) no
    /// engine is available at all — that last case reports `Storage` naming
    /// the missing feature, *not* [`DatabaseError::EngineUnavailable`],
    /// which no path here can construct: each [`Engine`] variant is gated on
    /// the feature compiling its own backend, so an engine an
    /// [`OpenOptions::engine`] caller can name is by construction compiled
    /// in (see that variant's own doc).
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
    /// [`DatabaseError::Sql`] if the engine rejects the statement;
    /// [`DatabaseError::Reentrant`] if the calling thread is already inside
    /// a [`Self::transaction`] closure on this same handle (use the
    /// [`Transaction`]'s own `execute` there).
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> Result<u64, DatabaseError> {
        let mut conn = self.lock_conn()?;
        conn.conn().execute(sql, &params.into_params())
    }

    /// Run a row-returning statement (`SELECT`), returning every resulting
    /// row.
    ///
    /// # Errors
    /// [`DatabaseError::Sql`] if the engine rejects the statement;
    /// [`DatabaseError::Reentrant`] if the calling thread is already inside
    /// a [`Self::transaction`] closure on this same handle (use the
    /// [`Transaction`]'s own `query` there).
    pub fn query(&self, sql: &str, params: impl IntoParams) -> Result<Vec<Row>, DatabaseError> {
        let mut conn = self.lock_conn()?;
        conn.conn().query(sql, &params.into_params())
    }

    /// Run `f` inside a `BEGIN`/`COMMIT`/`ROLLBACK` transaction — the same
    /// plain SQL on both engines, run through the engine seam. Commits on
    /// `Ok`, rolls back on `Err`, and returns whatever `f` returned (or its
    /// error).
    ///
    /// The transaction is rolled back on *every* path out that isn't a
    /// successful `COMMIT` — including the `COMMIT` statement itself failing
    /// (SQLite's deferred-constraint check runs there and leaves the
    /// transaction open) and `f` panicking — so a call always leaves the
    /// handle's connection ready for the next one, never stranded
    /// mid-transaction.
    ///
    /// `f` must not call `execute`/`query`/`transaction` on the same
    /// [`Database`] handle: the connection is already locked for the
    /// transaction's whole span, and re-entering it from the same thread is
    /// refused with [`DatabaseError::Reentrant`] rather than deadlocking.
    /// Use the [`Transaction`] handle `f` is given instead. Other threads
    /// calling this handle meanwhile are unaffected — they queue, per the
    /// module doc's *Threading model* section.
    ///
    /// # Errors
    /// `f`'s own error, if it returns `Err` (after rolling back). A
    /// `BEGIN`/`COMMIT`/`ROLLBACK` statement itself failing also surfaces
    /// as [`DatabaseError::Sql`] (again after rolling back).
    /// [`DatabaseError::Reentrant`] if the calling thread already holds this
    /// handle's connection.
    pub fn transaction<T>(
        &self,
        f: impl FnOnce(&Transaction) -> Result<T, DatabaseError>,
    ) -> Result<T, DatabaseError> {
        let mut locked = self.lock_conn()?;
        locked.conn().execute("BEGIN", &[])?;
        // Armed the moment BEGIN succeeds: from here on, *every* exit but a
        // successful COMMIT rolls back through this guard's `Drop` — `f`
        // returning `Err`, the COMMIT itself failing, and the case no match
        // arm can reach, `f` panicking (see [`RollbackGuard`]).
        let mut guard = RollbackGuard::new(locked.conn());
        // Scoped so `txn`'s borrow of the connection ends before the COMMIT
        // below — `Transaction` holds no resource of its own to release,
        // only this borrow.
        let result = {
            let txn = Transaction {
                conn: RefCell::new(guard.conn()),
            };
            f(&txn)
        };
        let value = result?;
        guard.conn().execute("COMMIT", &[])?;
        guard.disarm();
        Ok(value)
    }
}

/// The RAII lock on a [`Database`]'s connection: holds the `MutexGuard` and
/// clears the owner [`Database::lock_conn`] recorded in `holder`, so the
/// reentrancy check can never read a stale owner. `Drop` runs before the
/// `MutexGuard` field is dropped, so the owner is always cleared *before*
/// the mutex opens to the next thread.
struct ConnGuard<'a> {
    conn: MutexGuard<'a, Box<dyn EngineConn>>,
    holder: &'a AtomicU64,
}

impl ConnGuard<'_> {
    /// The locked connection. A method rather than a `DerefMut` impl
    /// because every caller wants the unboxed `&mut dyn EngineConn`, not
    /// the `Box`.
    fn conn(&mut self) -> &mut dyn EngineConn {
        &mut **self.conn
    }
}

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.holder.store(UNHELD, Ordering::Release);
    }
}

/// Arms a `ROLLBACK` over the span between a successful `BEGIN` and a
/// successful `COMMIT` — [`Database::transaction`]'s cleanup for every
/// abnormal exit, including the one no `match` arm can cover: the closure
/// **panicking**, which unwinds straight past any commit/rollback logic.
///
/// Leaving any of those paths without a rollback strands the shared
/// connection mid-transaction, and a `Database` handle outlives the call: a
/// later `transaction` then fails at `BEGIN`, and a later bare
/// `execute`/`query` silently *joins* the orphaned transaction and loses its
/// writes when the handle is dropped. [`Database::lock_conn`]'s poison
/// policy rests on this guard, too.
///
/// The rollback is best-effort (`let _ =`): an already-broken connection
/// must not hide the caller's real error, and a `Drop` running during an
/// unwind has nowhere to report to anyway.
struct RollbackGuard<'a> {
    conn: &'a mut dyn EngineConn,
    armed: bool,
}

impl<'a> RollbackGuard<'a> {
    /// Arm the guard over `conn` — the caller must already have run a
    /// successful `BEGIN` on it.
    fn new(conn: &'a mut dyn EngineConn) -> Self {
        Self { conn, armed: true }
    }

    /// The guarded connection, reborrowed for as long as the caller holds
    /// `&mut self`.
    fn conn(&mut self) -> &mut dyn EngineConn {
        &mut *self.conn
    }

    /// Disarm: the transaction ended on its own terms (a successful
    /// `COMMIT`), so `Drop` must not roll anything back.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RollbackGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.conn.execute("ROLLBACK", &[]);
        }
    }
}

// --- Reentrancy tokens --------------------------------------------------

/// [`Database::holder`]'s "no thread holds this connection" value —
/// [`thread_token`] never mints it.
const UNHELD: u64 = 0;

/// A process-unique, never-reused identifier for the calling thread,
/// allocated on that thread's first database call. Never [`UNHELD`].
///
/// `std::thread::ThreadId` is the natural source, but it is neither
/// storable in an atomic nor numerically readable on stable — its only
/// accessor, `ThreadId::as_u64`, is still unstable (`thread_id_value`,
/// rust-lang/rust#67939) — so this crate mints its own token. Tokens are
/// handed out monotonically and never recycled, so a token can only ever
/// name the one thread it was minted for, even after that thread exits.
fn thread_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(UNHELD + 1);
    thread_local! {
        static TOKEN: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TOKEN.with(|token| *token)
}

/// A handle to one open transaction, passed to [`Database::transaction`]'s
/// closure.
///
/// Borrows the parent [`Database`]'s already-locked connection for the
/// transaction's lifetime — `execute`/`query` take `&self` (via an
/// internal `RefCell`) so the closure can call either any number of times
/// without needing `&mut`.
///
/// This is the *only* way to run a statement inside the transaction: the
/// parent handle's own `execute`/`query`/`transaction` report
/// [`DatabaseError::Reentrant`] for the whole span (see
/// [`Database::transaction`]).
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

/// `<base>/databases/<name>.db`, with the legacy-base **read-through**
/// [`Database::open`] applies on macOS — factored out from
/// [`resolve_db_path`] over both base directories so it's testable against
/// temp dirs without ever touching a real data directory.
///
/// When the file under `base` does not exist and `legacy_base` is `Some`
/// (macOS only — see [`frust_paths::legacy_data_dir`]), the same
/// `databases/<name>.db` shape is checked under the legacy base and
/// returned **only if that file exists**; otherwise the `base` path wins.
/// Nothing is ever migrated, copied, or deleted: an existing database keeps
/// being opened where it already lives.
///
/// This crate cannot rely on `data_dir()`'s own built-in macOS fallback,
/// which probes `<base>/<app_stem>` — a `databases/` directory is
/// deliberately shared by every frust app on the machine and joins no
/// app-stem component, so that probe can never see it.
fn resolve_db_path_from(
    base: &Path,
    legacy_base: Option<&Path>,
    name: &str,
) -> Result<PathBuf, DatabaseError> {
    let path = db_file_path(base, name)?;
    // `if let Some(legacy_base) = legacy_base` first: on every non-macOS
    // target `legacy_base` is `None`, so the `&&`'s right-hand side (the
    // `try_exists` stat) never runs — genuinely syscall-free there, not
    // just discarded. `try_exists` (not `exists`) so a momentarily
    // unstat-able `path` (EACCES/ELOOP/dangling symlink) is treated as
    // PRESENT — never silently diverted to a stale legacy file; the
    // subsequent open surfaces the real error on the canonical path.
    if let Some(legacy_base) = legacy_base
        && !path.try_exists().unwrap_or(true)
    {
        let legacy_path = db_file_path(legacy_base, name)?;
        // `Err(_)` on the legacy probe means treat it as absent — never
        // divert onto a broken legacy path either.
        if legacy_path.try_exists().unwrap_or(false) {
            log::debug!(
                "frust-database: {} not found, opening legacy {}",
                path.display(),
                legacy_path.display()
            );
            return Ok(legacy_path);
        }
    }
    Ok(path)
}

/// The [`DatabaseError::Storage`] text for an unresolvable data directory — cfg-selected: Android names the nativeInitPlatform ordering, every other target keeps the HOME/APPDATA wording (pinned by a host test).
#[cfg(target_os = "android")]
const UNRESOLVED_DATA_DIR: &str = "could not resolve the app data directory: the host shell has not installed the Android directories yet (Database::open must run after FrustSurfaceView.nativeInitPlatform, i.e. from app code, not from a static initializer)";

#[cfg(not(target_os = "android"))]
const UNRESOLVED_DATA_DIR: &str = "could not resolve a user data directory (HOME/APPDATA unset)";

/// [`Database::open`]'s full path resolution: sanitize `name`, resolve the
/// user data directory, join this crate's standard `databases/<name>.db`
/// location (reading through to a legacy base where one exists — see
/// [`resolve_db_path_from`]), and ensure the resolved database's parent
/// directory exists.
///
/// On Android, the user data directory is `<Context.getFilesDir()>`, installed
/// by the platform shell's `nativeInitPlatform`. Returns [`DatabaseError::Storage`]
/// if the directory cannot be resolved (e.g. `Database::open` is called before
/// `nativeInitPlatform` completes).
fn resolve_db_path(name: &str) -> Result<PathBuf, DatabaseError> {
    let base = frust_paths::data_dir()
        .ok_or_else(|| DatabaseError::Storage(UNRESOLVED_DATA_DIR.into()))?;
    let path = resolve_db_path_from(&base, frust_paths::legacy_data_dir().as_deref(), name)?;
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

    // --- Legacy-base read-through (macOS shape, real temp dirs) ----------

    /// A unique scratch directory under the OS temp dir — the read-through
    /// tests below probe the real filesystem (`Path::try_exists`), so they
    /// must never touch a real data directory.
    ///
    /// Created eagerly with `fs::create_dir` (fails loudly if the name is
    /// already occupied, e.g. by a planted symlink) rather than left for a
    /// later `create_dir_all` to walk through silently.
    fn scratch_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-database-test-{}-{tag}-{n}",
            std::process::id()
        ));
        fs::create_dir(&dir).expect("scratch dir must not already exist (planted path?)");
        dir
    }

    /// Create `<base>/databases/<name>.db` with placeholder bytes and
    /// return its path — the on-disk shape `db_file_path` builds.
    fn seed_db_file(base: &Path, name: &str) -> PathBuf {
        let path = db_file_path(base, name).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"placeholder").unwrap();
        path
    }

    /// **Negative control for the whole fix**: an existing macOS install's
    /// database lives at `<legacy base>/databases/<name>.db` and there is
    /// no `<legacy base>/<app_stem>/` directory anywhere — the shape
    /// `frust_paths::data_dir()`'s own built-in probe looks for. Resolution
    /// must still find the legacy file. This fails if anyone "simplifies"
    /// the fallback back into an `<legacy>/<app_stem>` directory probe.
    #[test]
    fn resolve_db_path_from_reads_through_to_legacy_file_with_no_app_stem_dir() {
        let new_base = scratch_dir("legacy-read-through-new");
        let legacy_base = scratch_dir("legacy-read-through-legacy");
        let legacy_path = seed_db_file(&legacy_base, "app");

        // The new base is empty, and the legacy base holds *only*
        // `databases/app.db` — deliberately no `<app_stem>/` subdirectory.
        let stem_dir = legacy_base.join(frust_paths::app_stem());
        assert!(!stem_dir.exists(), "test fixture must have no app-stem dir");

        assert_eq!(
            resolve_db_path_from(&new_base, Some(&legacy_base), "app").unwrap(),
            legacy_path,
        );

        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// A database already at the new location wins, even when a legacy file
    /// also exists — read-through never overrides a live new-location file
    /// (and never migrates/deletes the legacy one).
    #[test]
    fn resolve_db_path_from_prefers_the_new_file_when_both_exist() {
        let new_base = scratch_dir("both-new");
        let legacy_base = scratch_dir("both-legacy");
        let new_path = seed_db_file(&new_base, "app");
        let legacy_path = seed_db_file(&legacy_base, "app");

        assert_eq!(
            resolve_db_path_from(&new_base, Some(&legacy_base), "app").unwrap(),
            new_path,
        );
        assert!(legacy_path.exists(), "legacy file must be left untouched");

        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// `Path::try_exists` (not `Path::exists`) semantics: a new-location
    /// probe that errors (here, `EACCES` from an unreadable ancestor
    /// directory) must never be treated as "absent" and diverted to a
    /// legacy file — `Err(_)` on the new-path probe means treat it as
    /// PRESENT, so the unresolvable path is returned unchanged and the
    /// subsequent `open` surfaces the real error. This fails under the old
    /// `Path::exists()` behaviour, which swallows the permission error into
    /// `false` and would wrongly divert to `legacy_path` below.
    #[test]
    #[cfg(unix)]
    fn resolve_db_path_from_does_not_divert_when_new_path_is_unstatable() {
        use std::os::unix::fs::PermissionsExt;

        let new_base = scratch_dir("unstatable-new");
        let locked_dir = new_base.join("locked");
        fs::create_dir(&locked_dir).unwrap();
        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o000)).unwrap();

        let legacy_base = scratch_dir("unstatable-legacy");
        let legacy_path = seed_db_file(&legacy_base, "app");

        let path = db_file_path(&locked_dir, "app").unwrap();
        if path.try_exists().is_ok() {
            // Running as root (or under some other permission-bypassing
            // capability): a 0o000 directory doesn't block traversal, so
            // this fixture can't produce the EACCES this test targets.
            // Restore permissions so cleanup below can actually remove the
            // directory, then skip rather than assert something the
            // fixture didn't exercise.
            fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
            let _ = fs::remove_dir_all(&new_base);
            let _ = fs::remove_dir_all(&legacy_base);
            return;
        }

        let resolved = resolve_db_path_from(&locked_dir, Some(&legacy_base), "app").unwrap();
        assert_eq!(
            resolved, path,
            "an unstatable new path must not divert to the legacy file"
        );
        assert_ne!(resolved, legacy_path);

        fs::set_permissions(&locked_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&new_base);
        let _ = fs::remove_dir_all(&legacy_base);
    }

    /// Neither file exists (a first run): the new-location path is chosen,
    /// so `resolve_db_path` creates the database where it belongs today.
    #[test]
    fn resolve_db_path_from_uses_the_new_path_when_neither_exists() {
        let new_base = scratch_dir("neither-new");
        let legacy_base = scratch_dir("neither-legacy");

        assert_eq!(
            resolve_db_path_from(&new_base, Some(&legacy_base), "app").unwrap(),
            db_file_path(&new_base, "app").unwrap(),
        );
    }

    /// `legacy_base: None` — every non-macOS target, where
    /// `frust_paths::legacy_data_dir()` returns `None` — resolves to the
    /// new path unconditionally.
    #[test]
    fn resolve_db_path_from_without_a_legacy_base_uses_the_new_path() {
        let new_base = scratch_dir("no-legacy");
        assert_eq!(
            resolve_db_path_from(&new_base, None, "app").unwrap(),
            db_file_path(&new_base, "app").unwrap(),
        );
    }

    /// Name sanitization still runs ahead of any read-through.
    #[test]
    fn resolve_db_path_from_rejects_an_invalid_name() {
        let new_base = scratch_dir("invalid-name");
        assert!(resolve_db_path_from(&new_base, None, "a/b").is_err());
        assert!(resolve_db_path_from(&new_base, None, "").is_err());
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

    // --- Error messages --------------------------------------------------

    #[test]
    fn unresolved_data_dir_is_non_empty() {
        assert!(!UNRESOLVED_DATA_DIR.is_empty());
    }

    #[test]
    #[cfg(not(target_os = "android"))]
    fn non_android_unresolved_data_dir_matches_legacy_wording() {
        assert_eq!(
            UNRESOLVED_DATA_DIR,
            "could not resolve a user data directory (HOME/APPDATA unset)"
        );
    }
}
