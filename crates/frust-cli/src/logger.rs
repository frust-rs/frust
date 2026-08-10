//! A minimal `log::Log` sink for `frust dap`, writing to stderr only.
//!
//! Mirrors `frust-shell-desktop::logger`/`frust-shell-ios::ffi_glue::StderrLogger`'s
//! shape (hand-rolled `log::Log` impl, `FRUST_LOG` env override, `writeln!` to
//! stderr tolerant of write failure), but with one deliberate difference:
//! **default level is `Warn`, not `Info`**. `frust dap` speaks the DAP wire
//! protocol over stdout in stdio mode, and stderr is the only diagnostic
//! channel an IDE client sees — a DAP server should stay quiet by default
//! rather than narrate every `log::info!` from the engine it drives
//! (`frust_mcp::engine::SessionEngine`) at an IDE client. `FRUST_LOG=info`
//! (or `debug`/`trace`) raises it the same way the other two shells' sink does.
//!
//! This sink NEVER writes stdout — that would corrupt the DAP wire protocol
//! in stdio mode (`docs/CLI_ARCHITECTURE.md`'s print-free contract for
//! `frust-dap`).

use std::io::Write;
use std::sync::Once;

/// A minimal `log::Log` writing to stderr, installed once in [`init_once`].
struct StderrLogger {
    level: log::LevelFilter,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            // A failed write to stderr is nothing we can act on here; drop it.
            let _ = writeln!(
                std::io::stderr(),
                "[frust {}] {}",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Install the stderr-only logger exactly once per process, so `log::warn!`/
/// `log::error!` calls anywhere in the `frust dap` graph (in particular
/// `frust-dap`'s 20+ diagnostic call sites) actually reach a human instead of
/// being silently discarded. Default level is `Warn` — deliberately quieter
/// than the desktop/iOS shells' `Info` default, since a DAP server must stay
/// quiet to IDE clients unless asked otherwise; `FRUST_LOG` (e.g. `info`,
/// `debug`, `trace`, `warn`) overrides it, same convention as those two.
pub fn init_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        let level = std::env::var("FRUST_LOG")
            .ok()
            .and_then(|raw| raw.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Warn);
        // Leak the logger so it satisfies `set_logger`'s `&'static` bound; it
        // lives for the whole process, installed at most once.
        let logger: &'static StderrLogger = Box::leak(Box::new(StderrLogger { level }));
        if log::set_logger(logger).is_ok() {
            log::set_max_level(level);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;

    #[test]
    fn enabled_respects_level_filter() {
        let logger = StderrLogger {
            level: log::LevelFilter::Warn,
        };
        let warn_meta = log::MetadataBuilder::new().level(log::Level::Warn).build();
        let info_meta = log::MetadataBuilder::new().level(log::Level::Info).build();
        assert!(logger.enabled(&warn_meta));
        assert!(!logger.enabled(&info_meta));
    }

    #[test]
    fn init_once_is_idempotent() {
        // Calling twice must not panic (log::set_logger errors are swallowed).
        init_once();
        init_once();
    }
}
