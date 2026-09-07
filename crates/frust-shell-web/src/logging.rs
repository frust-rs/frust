//! Browser console logging (w1-03 owns this module).
//!
//! stderr is a silent no-op in a browser, so the shell's `log` facade must be
//! routed to `console.*` (`console_log`) and panics to `console.error`
//! (`console_error_panic_hook`). This module is seeded empty by the conductor
//! so the crate's module tree and dependency rows exist ahead of the card
//! that fills it in; nothing here is public API yet.
