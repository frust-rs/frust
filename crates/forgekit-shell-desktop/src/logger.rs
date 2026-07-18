//! A minimal `log::Log` sink for the desktop shell (spec §14 phase 7, task
//! 10), writing to stderr.
//!
//! Rationale: `cargo run`'s stdout/stderr is already captured directly by the
//! terminal/CI runner, so a stderr sink is enough — no new dependency
//! (`env_logger`, `tracing-subscriber`, ...) is needed just to make
//! `forgekit-shell-common::perf`'s `log::info!` startup-span/frame-stats
//! lines visible. Mirrors `forgekit-shell-ios::ffi_glue::StderrLogger`
//! exactly (same `FORGEKIT_LOG` override, same default level, same line
//! format) so the two shells' perf output reads identically — desktop was
//! the one shell with no logger installed at all before this (Android's
//! `android_logger`, iOS's `StderrLogger` both predate this task; see
//! `docs/ARCHITECTURE.md`'s Module Structure for why each shell owns its own
//! platform-appropriate sink).
//!
//! Installing this logger is orthogonal to whether anything is actually
//! logged: `perf::enabled()` (compile-time `FORGEKIT_TRACE` define or
//! runtime env var) gates every `FrameStats`/`StartupSpans` recording and
//! `emit_log` call to a no-op when tracing is off, so a plain `cargo run`
//! still prints nothing even with this sink installed.

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
                "[forgekit {}] {}",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// Install the stderr logger exactly once per process, so `log::*` calls from
/// any crate in the graph (in particular `forgekit-shell-common::perf`'s
/// `log::info!` lines) reach the terminal. Default level is `Info`;
/// `FORGEKIT_LOG` (e.g. `debug`, `trace`, `warn`) overrides it — the same
/// convention `forgekit-shell-ios`'s logger uses.
pub fn init_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        let level = std::env::var("FORGEKIT_LOG")
            .ok()
            .and_then(|raw| raw.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        // Leak the logger so it satisfies `set_logger`'s `&'static` bound; it
        // lives for the whole process, installed at most once.
        let logger: &'static StderrLogger = Box::leak(Box::new(StderrLogger { level }));
        if log::set_logger(logger).is_ok() {
            log::set_max_level(level);
        }
    });
}
