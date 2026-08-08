//! Stub `engine-turso` backend.
//!
//! **Placeholder only.** The `engine-turso` Cargo feature this module is
//! gated behind has no real dependencies yet (see `Cargo.toml`'s comment)
//! and nothing turns it on by default, so this file never actually
//! compiles in a normal build — it exists only so `mod turso;`
//! (`lib.rs`, written once by the crate-skeleton task) resolves to a real
//! file: `rustfmt` resolves every `mod` statement's target file
//! textually, regardless of its `#[cfg]`, so an absent file breaks
//! `cargo fmt --check` even though the module is otherwise inert. The
//! turso-backend task replaces this wholesale with a real async-bridged
//! connection.

use crate::engine::{EngineConn, Target};
use crate::{DatabaseError, Row, Value};

/// Placeholder connection handle — never actually opens a turso database.
pub(crate) struct TursoConn;

fn unimplemented() -> DatabaseError {
    DatabaseError::Sql {
        message: "unimplemented".into(),
    }
}

/// Always fails; the turso-backend task replaces this with a real
/// async-bridged connection open.
pub(crate) fn open(_target: Target) -> Result<TursoConn, DatabaseError> {
    Err(unimplemented())
}

impl EngineConn for TursoConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, DatabaseError> {
        Err(unimplemented())
    }

    fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<Row>, DatabaseError> {
        Err(unimplemented())
    }
}
