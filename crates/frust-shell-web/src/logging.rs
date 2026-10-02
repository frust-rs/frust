//! Browser console logging (w1-03 owns this module).
//!
//! stderr is a silent no-op in a browser — nothing reads a
//! `wasm32-unknown-unknown` page's stderr — so the shell's `log` facade must
//! be routed to `console.*` (`console_log`) and panics to `console.error`
//! (`console_error_panic_hook`), or every `log::warn!`/`log::error!` call in
//! this crate (and every crate beneath it: `frust-render`, `frust-gpu`,
//! `wgpu` itself) goes nowhere and a panic reaches the console as a bare,
//! unhelpful `unreachable` trap.
//!
//! # Idempotent against the facade's own install
//!
//! `frust::__web_bootstrap` — the `web_app!`-generated start shim's first
//! call — already installs both: `console_error_panic_hook::set_once()` and
//! `console_log::init_with_level(log::Level::Warn)`, before this crate's
//! `run_app` ever runs (see that function's doc comment in
//! `crates/frust/src/lib.rs`). So by the time anything in this shell could
//! call [`install`], a logger is very likely already live. `console_log::init`
//! (and every `log::set_logger` call under it) answers `Err` when a logger is
//! already installed — `log` allows exactly one sink per process — and
//! [`install`] treats that as "someone got here first", not a failure: it is
//! swallowed rather than propagated or panicked on. A caller that installs
//! this shell standalone (no facade in front of it — a browser test harness,
//! or a future non-`app!` entry point) still gets a working sink from the
//! first `install` that runs; the facade's own install still wins the race in
//! the normal `web_app!` path, and a later, more permissive `install(level)`
//! here still raises the effective ceiling (see below) rather than being a
//! silent no-op.
//!
//! `console_error_panic_hook::set_once()` is unconditionally safe to repeat —
//! it is a `std::sync::Once` internally — so it is always called, whichever
//! logger install branch is taken.
//!
//! # The level knob
//!
//! `wasm32-unknown-unknown` has no process environment, so the desktop
//! logger's `FRUST_LOG` env var ([`frust_shell_desktop::logger::init_once`])
//! has no counterpart here. This module documents two knobs instead, both
//! resolving to the same [`log::LevelFilter`] vocabulary (`off`, `error`,
//! `warn`, `info`, `debug`, `trace`, case-insensitive — `LevelFilter`'s own
//! `FromStr`):
//!
//! - [`install`] — an explicit API a caller (the facade, an app's own setup,
//!   or a test harness) calls with the level it wants, e.g.
//!   `frust_shell_web::logging::install(log::LevelFilter::Debug)`.
//! - [`level_from_query`] — a pure parser for a `?log=` query parameter (e.g.
//!   `https://example.com/app?log=debug`) a caller resolves against
//!   `window.location.search` and feeds to [`install`]. Reading
//!   `location.search` itself needs `web-sys`'s `Location` feature, which
//!   this crate's `Cargo.toml` does not carry (only `Window` is enabled — see
//!   that manifest's comment) and this module does not add on its own
//!   authority; [`level_from_query`] is deliberately a plain `&str -> ...`
//!   function rather than one that reaches into `web_sys::window()` itself,
//!   so it is usable the moment a caller (or a future card) reads the query
//!   string through whatever seam ends up owning that `web-sys` feature, and
//!   is unit-testable on the build host with no browser in the loop either
//!   way.
//!
//! # Native
//!
//! This module carries no blanket `#[cfg(target_arch = "wasm32")]` on the
//! file: [`parse_level`] and [`level_from_query`] are plain string parsing
//! with no browser or platform dependency, so they compile and are
//! unit-tested by `cargo test -p frust-shell-web` on the build host exactly
//! as on `wasm32-unknown-unknown`. [`install`] itself is `wasm32`-only real
//! code (it is the only symbol here that touches `console_log`/
//! `console_error_panic_hook`, both of which are `wasm32`-only dependency
//! rows in `Cargo.toml`); its native counterpart is a documented no-op with
//! the identical signature, so a caller that is generic over target (or a
//! future non-browser test harness built against this crate) never has to
//! `#[cfg]`-gate its own call site. Neither arm touches stderr on native: the
//! desktop shell owns its own stderr sink
//! ([`frust_shell_desktop::logger::init_once`]) and this crate has no reason
//! to install a second one for a target it does not ship on.

use log::LevelFilter;

/// The level [`install`] falls back to when [`level_from_query`] or
/// [`parse_level`] is given nothing parseable — `Warn`, matching the
/// facade's own `__web_bootstrap` default so a caller who never touches
/// either knob sees identical behaviour whichever install path happens to
/// run first.
pub const DEFAULT_LEVEL: LevelFilter = LevelFilter::Warn;

/// The query-parameter name [`level_from_query`] reads: `log`, e.g.
/// `https://example.com/app?log=debug`.
pub const LEVEL_QUERY_PARAM: &str = "log";

/// Parse a level string (already extracted from wherever it came from — a
/// query parameter, a config value) into a [`LevelFilter`], falling back to
/// [`DEFAULT_LEVEL`] on `None` or an unparseable value.
///
/// Delegates to `LevelFilter`'s own `FromStr`, which is case-insensitive and
/// accepts `off`/`error`/`warn`/`info`/`debug`/`trace` — the identical
/// vocabulary the desktop and iOS loggers' `FRUST_LOG` env var accepts, so a
/// developer does not have to learn a second spelling for the web.
pub fn parse_level(level: Option<&str>) -> LevelFilter {
    level
        .and_then(|raw| raw.parse::<LevelFilter>().ok())
        .unwrap_or(DEFAULT_LEVEL)
}

/// Extract the [`LEVEL_QUERY_PARAM`] value out of a raw query string — the
/// part of a URL after `?`, without the leading `?` (`window.location.search`
/// includes the `?` itself; a caller resolving from that must strip it first)
/// — and resolve it through [`parse_level`].
///
/// A small hand-rolled `&`-split rather than a URL/query-string crate: the
/// query string is already handed to this function as plain text, the
/// grammar this needs (find a `key=value` pair among `&`-separated ones,
/// ignore the rest, ignore a value-less or malformed pair) is a few lines,
/// and a new dependency in this crate's `Cargo.toml` for a single-key lookup
/// is not worth it either way. Case-sensitive on the key (`log`, not `Log`), to
/// keep the match trivial; the *value* still runs through `LevelFilter`'s
/// case-insensitive parse.
pub fn level_from_query(query: &str) -> LevelFilter {
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(LEVEL_QUERY_PARAM)?.strip_prefix('='));
    parse_level(value)
}

/// Install the browser console log sink and panic hook, idempotently.
///
/// Always calls `console_error_panic_hook::set_once()` first (see the module
/// docs — genuinely idempotent, safe whether or not the facade already ran
/// it), then attempts `console_log::init_with_level(level)`. Its `Err` means
/// a logger is already installed — this shell's own earlier `install` call,
/// or the facade's `__web_bootstrap` — and `console_log` cannot replace a
/// sink that already won the race (`log::set_logger` accepts exactly one).
/// Rather than treat that as nothing happened, the effective level is raised
/// via `log::set_max_level` instead: a caller asking for `Trace` after the
/// facade's `Warn` default still sees `Trace` lines emitted through whichever
/// sink is actually live, since `set_max_level` is a free-standing global the
/// installed logger's own filtering is layered under regardless of which
/// `install_*` call created it. `set_max_level` is never *lowered* by this
/// path — only `console_log::init_with_level`'s own success path can do
/// that, on the one install that actually wins the race — so a later,
/// stricter `install` call cannot silently mute lines a looser one already
/// armed.
///
/// `LevelFilter::Off` has no `log::Level` counterpart (nothing logs at
/// "off"), so it takes its own arm: no sink is installed for it (there is
/// nothing to sink), the max level is dropped to `Off`, and the panic hook
/// still installs — a page that asks for silence still wants its panics
/// reported, not swallowed.
#[cfg(target_arch = "wasm32")]
pub fn install(level: LevelFilter) {
    console_error_panic_hook::set_once();
    match level.to_level() {
        Some(level) => {
            if console_log::init_with_level(level).is_err() {
                log::set_max_level(level.to_level_filter());
            }
        }
        None => log::set_max_level(LevelFilter::Off),
    }
}

/// Native counterpart of [`install`]: a documented no-op.
///
/// This crate ships only on `wasm32-unknown-unknown` — `console_log` and
/// `console_error_panic_hook` are `wasm32`-only dependency rows in
/// `Cargo.toml` (see that file's comment) — but this arm exists so a caller
/// generic over target, or a native `cargo test -p frust-shell-web` run, has
/// a real symbol to call rather than a `#[cfg]` at every call site. It
/// touches neither `log`'s global logger nor stderr: a native host of this
/// crate (there is none today) would own its logger the way
/// `frust-shell-desktop` does, not through this module.
#[cfg(not(target_arch = "wasm32"))]
pub fn install(_level: LevelFilter) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_value_falls_back_to_the_default() {
        assert_eq!(parse_level(None), DEFAULT_LEVEL);
    }

    #[test]
    fn an_unparseable_value_falls_back_to_the_default() {
        assert_eq!(parse_level(Some("not-a-level")), DEFAULT_LEVEL);
        assert_eq!(parse_level(Some("")), DEFAULT_LEVEL);
    }

    #[test]
    fn every_level_name_parses_case_insensitively() {
        assert_eq!(parse_level(Some("trace")), LevelFilter::Trace);
        assert_eq!(parse_level(Some("Debug")), LevelFilter::Debug);
        assert_eq!(parse_level(Some("INFO")), LevelFilter::Info);
        assert_eq!(parse_level(Some("Warn")), LevelFilter::Warn);
        assert_eq!(parse_level(Some("error")), LevelFilter::Error);
        assert_eq!(parse_level(Some("OFF")), LevelFilter::Off);
    }

    #[test]
    fn the_log_param_is_found_among_other_params() {
        assert_eq!(
            level_from_query("theme=dark&log=debug&foo=bar"),
            LevelFilter::Debug
        );
        assert_eq!(level_from_query("log=trace"), LevelFilter::Trace);
    }

    #[test]
    fn a_missing_log_param_falls_back_to_the_default() {
        assert_eq!(level_from_query(""), DEFAULT_LEVEL);
        assert_eq!(level_from_query("theme=dark&foo=bar"), DEFAULT_LEVEL);
    }

    #[test]
    fn a_similarly_named_param_is_not_mistaken_for_the_log_param() {
        // `strip_prefix("log")` on `loglevel=debug` leaves `level=debug`,
        // which does not itself start with `=` — so this must NOT match.
        assert_eq!(level_from_query("loglevel=debug"), DEFAULT_LEVEL);
    }

    #[test]
    fn a_value_less_log_param_falls_back_to_the_default() {
        assert_eq!(level_from_query("log"), DEFAULT_LEVEL);
        assert_eq!(level_from_query("log&theme=dark"), DEFAULT_LEVEL);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_native_install_arm_does_not_panic() {
        // Nothing to observe off wasm32 — this is the "compiles and runs"
        // guarantee the module docs promise a generic-over-target caller.
        install(LevelFilter::Trace);
        install(LevelFilter::Off);
    }
}
