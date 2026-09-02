//! A minimal `log::Log` sink for the desktop shell, writing to stderr.
//!
//! Rationale: `cargo run`'s stdout/stderr is already captured directly by the
//! terminal/CI runner, so a stderr sink is enough — no new dependency
//! (`env_logger`, `tracing-subscriber`, ...) is needed just to make
//! `frust-shell-common::perf`'s `log::info!` startup-span/frame-stats
//! lines visible. Mirrors `frust-shell-ios::ffi_glue::StderrLogger`
//! exactly (same `FRUST_LOG` override, same default level, same line
//! format) so the two shells' perf output reads identically — desktop was
//! the one shell with no logger installed at all before this (Android's
//! `android_logger`, iOS's `StderrLogger` both predate this one; see
//! `docs/ARCHITECTURE.md`'s Module Structure for why each shell owns its own
//! platform-appropriate sink).
//!
//! Installing this logger is orthogonal to whether anything is actually
//! logged: `perf::enabled()` (compile-time `FRUST_TRACE` define or
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
        // Keep enabled() permissive — filtering is done in log() where the
        // message content is available.
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

/// Install the stderr logger exactly once per process, so `log::*` calls from
/// any crate in the graph (in particular `frust-shell-common::perf`'s
/// `log::info!` lines) reach the terminal. Default level is `Info`;
/// `FRUST_LOG` (e.g. `debug`, `trace`, `warn`) overrides it — the same
/// convention `frust-shell-ios`'s logger uses.
///
/// Also emits one unconditional (NOT `perf-trace`-gated) `log::info!` marker
/// line — `scripts/release-lean-check.sh`'s contrast check needs a
/// first-party info-level string that is guaranteed present in a normal
/// (profile) build and constant-folded out under the release `lean` feature's
/// `log/release_max_level_warn` ceiling, since every existing info-level site
/// is either `perf-trace`-gated or mobile-shell-only.
pub fn init_once() {
    static LOGGER: Once = Once::new();
    LOGGER.call_once(|| {
        let level = std::env::var("FRUST_LOG")
            .ok()
            .and_then(|raw| raw.parse::<log::LevelFilter>().ok())
            .unwrap_or(log::LevelFilter::Info);
        // Leak the logger so it satisfies `set_logger`'s `&'static` bound; it
        // lives for the whole process, installed at most once.
        let logger: &'static StderrLogger = Box::leak(Box::new(StderrLogger { level }));
        if log::set_logger(logger).is_ok() {
            log::set_max_level(level);
        }
        log::info!("frust-shell-desktop: logger initialized");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::{Level, Log, Metadata, MetadataBuilder};

    fn metadata(target: &str, level: Level) -> Metadata<'_> {
        MetadataBuilder::new().target(target).level(level).build()
    }

    #[test]
    fn targets_are_never_suppressed() {
        // `enabled` is the only filter `StderrLogger` applies, and it reads
        // only the record's level — no target (however it looks) is ever
        // treated specially.
        let logger = StderrLogger {
            level: log::LevelFilter::Info,
        };
        for target in [
            "some_gpu_crate::scene",
            "frust_shell_common::perf",
            "some_other_crate",
        ] {
            let meta = metadata(target, Level::Warn);
            assert!(
                logger.enabled(&meta),
                "{target} at Warn should be enabled under an Info filter"
            );
        }
    }
}
