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
//!
//! One target-specific carve-out: `vello`'s own `log::error!`/`log::warn!`
//! calls are suppressed at the default level (see
//! [`StderrLogger::enabled`]) — vello 0.9's bitmap-emoji decode path can log
//! an unfixed-upstream error once per paint for a glyph it can't decode
//! (`workflow/plans/features/device-parity/tasks/15-emoji-colortype.md`,
//! linebender/vello#1031), which would otherwise spam every frame. Pass
//! `FORGEKIT_LOG=debug` (or `trace`) to see vello's own log lines again when
//! actually debugging the render stack.

use std::io::Write;
use std::sync::Once;

/// Target prefix for vello's own `log` calls (`vello::scene`, etc.) — see
/// [`StderrLogger::enabled`]'s vello-noise carve-out below.
const VELLO_TARGET_PREFIX: &str = "vello";

/// A minimal `log::Log` writing to stderr, installed once in [`init_once`].
struct StderrLogger {
    level: log::LevelFilter,
}

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // vello 0.9's bitmap-emoji decode path (`scene.rs`'s
        // `output_color_type` check, see
        // `workflow/plans/features/device-parity/tasks/15-emoji-colortype.md`)
        // logs an `error!`/`warn!` once *per paint* for any glyph it can't
        // decode (upstream, unfixed: linebender/vello#1031) — every default
        // level (`Info` and below) would otherwise print that line every
        // frame for as long as the affected glyph stays on screen. Suppress
        // vello's own Error/Warn noise unless `FORGEKIT_LOG` explicitly asks
        // for `debug`/`trace` (i.e. the caller is deliberately debugging the
        // render stack, not just running the app).
        if metadata.target().starts_with(VELLO_TARGET_PREFIX)
            && metadata.level() <= log::Level::Warn
            && self.level < log::LevelFilter::Debug
        {
            return false;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use log::{Level, Log, Metadata, MetadataBuilder};

    fn metadata(target: &str, level: Level) -> Metadata<'_> {
        MetadataBuilder::new().target(target).level(level).build()
    }

    #[test]
    fn vello_error_suppressed_at_default_level() {
        let logger = StderrLogger {
            level: log::LevelFilter::Info,
        };
        assert!(!logger.enabled(&metadata("vello::scene", Level::Error)));
        assert!(!logger.enabled(&metadata("vello::scene", Level::Warn)));
    }

    #[test]
    fn vello_error_shown_when_debug_requested() {
        let logger = StderrLogger {
            level: log::LevelFilter::Debug,
        };
        assert!(logger.enabled(&metadata("vello::scene", Level::Error)));
    }

    #[test]
    fn vello_info_and_below_unaffected() {
        let logger = StderrLogger {
            level: log::LevelFilter::Info,
        };
        // Only the Error/Warn noise carve-out applies; vello Info lines
        // still follow the normal level filter.
        assert!(logger.enabled(&metadata("vello::scene", Level::Info)));
        assert!(!logger.enabled(&metadata("vello::scene", Level::Debug)));
    }

    #[test]
    fn non_vello_targets_unaffected() {
        let logger = StderrLogger {
            level: log::LevelFilter::Info,
        };
        assert!(logger.enabled(&metadata("forgekit_shell_common::perf", Level::Error)));
        assert!(logger.enabled(&metadata("forgekit_shell_common::perf", Level::Info)));
    }
}
