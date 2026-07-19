//! A minimal `log::Log` sink for the desktop shell (spec §14 phase 7, task
//! 10), writing to stderr.
//!
//! Rationale: `cargo run`'s stdout/stderr is already captured directly by the
//! terminal/CI runner, so a stderr sink is enough — no new dependency
//! (`env_logger`, `tracing-subscriber`, ...) is needed just to make
//! `frust-shell-common::perf`'s `log::info!` startup-span/frame-stats
//! lines visible. Mirrors `frust-shell-ios::ffi_glue::StderrLogger`
//! exactly (same `FRUST_LOG` override, same default level, same line
//! format) so the two shells' perf output reads identically — desktop was
//! the one shell with no logger installed at all before this (Android's
//! `android_logger`, iOS's `StderrLogger` both predate this task; see
//! `docs/ARCHITECTURE.md`'s Module Structure for why each shell owns its own
//! platform-appropriate sink).
//!
//! Installing this logger is orthogonal to whether anything is actually
//! logged: `perf::enabled()` (compile-time `FRUST_TRACE` define or
//! runtime env var) gates every `FrameStats`/`StartupSpans` recording and
//! `emit_log` call to a no-op when tracing is off, so a plain `cargo run`
//! still prints nothing even with this sink installed.
//!
//! One target-specific carve-out: vello 0.9's bitmap-emoji decode path logs
//! certain unfixed-upstream errors/warnings (e.g. "Unsupported `output_color_type`",
//! "Invalid PNG in font") once per paint for glyphs it can't decode
//! (`workflow/plans/features/device-parity/tasks/15-emoji-colortype.md`,
//! linebender/vello#1031). These specific messages are suppressed at the default
//! level to avoid per-frame spam; other vello errors/warnings still surface. Pass
//! `FRUST_LOG=debug` (or `trace`) to see all vello log lines again when
//! actually debugging the render stack.

use std::io::Write;
use std::sync::Once;

/// Target prefix for vello's own `log` calls (`vello::scene`, etc.) — see
/// [`StderrLogger::log`]'s vello-noise carve-out below.
const VELLO_TARGET_PREFIX: &str = "vello";

/// Known-noisy vello error/warning messages that should be suppressed at the
/// default log level to avoid per-frame spam. These come from unfixed-upstream
/// vello issues (e.g. linebender/vello#1031). Other vello errors/warnings still
/// surface even below debug level.
const VELLO_NOISY_MESSAGES: &[&str] = &["Unsupported `output_color_type`", "Invalid PNG in font"];

/// Returns true if this record should be suppressed (not logged), false otherwise.
/// Only known-noisy vello messages are suppressed, at levels below debug.
fn should_suppress_record(
    metadata: &log::Metadata,
    level_filter: log::LevelFilter,
    message: &str,
) -> bool {
    // Only suppress specific known-noisy vello Error/Warn messages
    // below debug level to avoid per-frame spam.
    metadata.target().starts_with(VELLO_TARGET_PREFIX)
        && metadata.level() <= log::Level::Warn
        && level_filter < log::LevelFilter::Debug
        && VELLO_NOISY_MESSAGES
            .iter()
            .any(|&noisy| message.contains(noisy))
}

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
            let message = format!("{}", record.args());
            if should_suppress_record(record.metadata(), self.level, &message) {
                // This is a known-noisy vello message — suppress it
                return;
            }
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::{Level, Metadata, MetadataBuilder};

    fn metadata(target: &str, level: Level) -> Metadata<'_> {
        MetadataBuilder::new().target(target).level(level).build()
    }

    #[test]
    fn known_noisy_vello_error_suppressed_at_default_level() {
        let vello_meta = metadata("vello::scene", Level::Error);
        let noisy_msg = "Unsupported `output_color_type`";
        // At Info level (below debug), known-noisy messages are suppressed
        assert!(should_suppress_record(
            &vello_meta,
            log::LevelFilter::Info,
            noisy_msg
        ));

        // The "Invalid PNG in font" message is also suppressed
        let png_msg = "Invalid PNG in font";
        assert!(should_suppress_record(
            &vello_meta,
            log::LevelFilter::Info,
            png_msg
        ));
    }

    #[test]
    fn known_noisy_vello_error_shown_when_debug_requested() {
        let vello_meta = metadata("vello::scene", Level::Error);
        let noisy_msg = "Unsupported `output_color_type`";
        // At Debug level, even known-noisy messages are shown
        assert!(!should_suppress_record(
            &vello_meta,
            log::LevelFilter::Debug,
            noisy_msg
        ));
    }

    #[test]
    fn unknown_vello_error_shown_at_default_level() {
        let vello_meta = metadata("vello::scene", Level::Error);
        let unknown_msg = "Some other vello error that we haven't seen before";
        // Unknown vello errors are not suppressed, even at default level
        assert!(!should_suppress_record(
            &vello_meta,
            log::LevelFilter::Info,
            unknown_msg
        ));
    }

    #[test]
    fn vello_info_and_below_unaffected() {
        let vello_info = metadata("vello::scene", Level::Info);
        let vello_debug = metadata("vello::scene", Level::Debug);
        // Info level is not suppressed (suppression only applies to Error/Warn)
        assert!(!should_suppress_record(
            &vello_info,
            log::LevelFilter::Info,
            "Unsupported `output_color_type`"
        ));
        // Debug level messages follow normal filtering (level too low)
        assert!(!should_suppress_record(
            &vello_debug,
            log::LevelFilter::Info,
            "Unsupported `output_color_type`"
        ));
    }

    #[test]
    fn non_vello_targets_unaffected() {
        let perf_error = metadata("frust_shell_common::perf", Level::Error);
        // Non-vello targets are never suppressed, even with noisy-sounding messages
        assert!(!should_suppress_record(
            &perf_error,
            log::LevelFilter::Info,
            "Unsupported `output_color_type`"
        ));
    }
}
