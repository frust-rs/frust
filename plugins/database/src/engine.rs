//! The engine seam: the crate-private trait every backend module
//! implements, plus the dispatcher [`crate::Database`] calls through — see
//! this crate's module doc (`lib.rs`)'s *Threading model* section.
//!
//! Crate-private end to end: nothing here is reachable outside
//! `frust-database`. A backend module (`sqlite`, `turso`) implements
//! [`EngineConn`] once; [`open_conn`] is the one place that picks which
//! backend module's `open` a [`crate::Engine`] value routes to.

use std::path::PathBuf;

use crate::{DatabaseError, Engine, Row, Value};

/// One open connection to a SQL engine, held behind
/// [`crate::Database`]'s `Mutex<Box<dyn EngineConn>>` — one serialized
/// connection per handle (`lib.rs`'s *Threading model* doc). `Send` (not
/// `Sync`): a connection is only ever touched through that `Mutex`, never
/// shared bare across threads.
pub(crate) trait EngineConn: Send {
    /// Run a non-row-returning statement (`INSERT`/`UPDATE`/`DELETE`/DDL),
    /// returning the number of rows affected.
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, DatabaseError>;
    /// Run a row-returning statement (`SELECT`), returning every resulting
    /// row.
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Row>, DatabaseError>;
}

/// Where an [`EngineConn`] opens against.
///
/// `Path`'s inner `PathBuf` is unread today — `sqlite.rs`'s stub `open`
/// ignores its `target` entirely (see its module doc); the sqlite-backend
/// task's real implementation is this field's first reader.
#[allow(
    dead_code,
    reason = "first real reader lands with the sqlite-backend task"
)]
pub(crate) enum Target {
    /// An on-disk database file.
    Path(PathBuf),
    /// A private, non-shared in-memory database
    /// ([`crate::Database::open_in_memory`]).
    Memory,
}

/// Open a connection for `engine` against `target`, dispatching to the one
/// compiled backend module for that engine.
///
/// Exhaustive over exactly the [`Engine`] variants compiled into this
/// build: with neither `engine-sqlite` nor `engine-turso` on, [`Engine`]
/// has no values at all (each variant is itself feature-gated — see
/// `lib.rs`'s `Engine` doc), so this function can never actually be
/// called in that configuration (nothing can construct an `Engine` to
/// pass it) — the match below is still exhaustive with zero arms in that
/// case, which is why `target` is allowed to go unused there.
#[allow(
    unused_variables,
    reason = "target is unused when Engine is uninhabited (no engine feature compiled)"
)]
pub(crate) fn open_conn(
    engine: Engine,
    target: Target,
) -> Result<Box<dyn EngineConn>, DatabaseError> {
    match engine {
        #[cfg(feature = "engine-sqlite")]
        Engine::Sqlite => crate::sqlite::open(target).map(|c| Box::new(c) as Box<dyn EngineConn>),
        #[cfg(feature = "engine-turso")]
        Engine::Turso => crate::turso::open(target).map(|c| Box::new(c) as Box<dyn EngineConn>),
    }
}
