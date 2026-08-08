//! Stub `engine-sqlite` backend.
//!
//! **Replaced wholesale by the sqlite-backend task.** This file exists
//! only so `frust-database`'s module wiring (`lib.rs`, written once by the
//! crate-skeleton task) compiles green this wave — every call reports
//! `DatabaseError::Sql` naming itself unimplemented rather than opening a
//! real `rusqlite::Connection`.

use crate::engine::{EngineConn, Target};
use crate::{DatabaseError, Row, Value};

/// Placeholder connection handle — never actually opens `rusqlite`.
pub(crate) struct SqliteConn;

fn unimplemented() -> DatabaseError {
    DatabaseError::Sql {
        message: "unimplemented".into(),
    }
}

/// Always fails; the sqlite-backend task replaces this with a real
/// `rusqlite::Connection::open`/`open_in_memory` call.
pub(crate) fn open(_target: Target) -> Result<SqliteConn, DatabaseError> {
    Err(unimplemented())
}

impl EngineConn for SqliteConn {
    fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, DatabaseError> {
        Err(unimplemented())
    }

    fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<Row>, DatabaseError> {
        Err(unimplemented())
    }
}
