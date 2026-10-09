//! iOS simulator patch-load probe: the `src/lib.rs` that `probe.sh` drops into a throwaway
//! `frust create` app (see README.md beside this file).
//!
//! The root component's `init` runs once at startup, before the first frame, and tries to load a
//! code library delivered after install (built from `patch/`) from the app's own data container.
//! `probe.sh` copies four files into `frust_paths::data_dir()` (`<data container>/Library/
//! Application Support`), one per strategy:
//!
//! - `plain-dlopen`: `libfrust_probe_unsigned.dylib`, the dylib with its signature removed
//!   (`codesign --remove-signature`; arm64 `ld` otherwise leaves a linker ad-hoc signature),
//!   opened with `frust_hotpatch::load_patch_library` (plain `dlopen` through libloading off
//!   Android).
//! - `linker-signed`: `libfrust_probe_linker.dylib`, the dylib exactly as cargo / `ld` wrote it
//!   (the linker ad-hoc signature, `flags=0x20002(adhoc,linker-signed)`), opened the same way.
//! - `adhoc-signed`: `libfrust_probe_adhoc.dylib`, the same dylib after `codesign -f -s -`, opened
//!   the same way.
//! - `negative-control`: `libfrust_probe_control.dylib`, 11 bytes of text. The loader's own error
//!   must reach the log, which proves a failed load is visible at all.
//!
//! A loaded library is resolved for `frust_probe_value` and the symbol is called; it returns 42.
//! Each strategy logs one line at info level, which the iOS shell's stderr logger prints and
//! `simctl launch --console-pty` carries: `frust-probe: strategy=<name> result=<value>` or
//! `frust-probe: strategy=<name> error=<loader error>`. Before each attempt it logs a
//! `frust-probe-attempt:` line with the path and the process id, so a process killed during a load
//! still leaves a trace and `probe.sh` can inspect the live process with `vmmap`.
//!
//! `FRUST_PROBE_STRATEGY` (set by `probe.sh` as `SIMCTL_CHILD_FRUST_PROBE_STRATEGY`) limits a
//! launch to one strategy, so every load runs in a fresh process; unset, all four run in order.
//! The probe tests LOADING only: no jump table, no relocation against the running app.

use std::path::{Path, PathBuf};

use frust::{Component, EdgeInsets, Padding, View, column, safe_area, text};

/// Every strategy, in run order, with the file `probe.sh` delivers for it.
const STRATEGIES: [(&str, &str); 4] = [
    ("plain-dlopen", "libfrust_probe_unsigned.dylib"),
    ("linker-signed", "libfrust_probe_linker.dylib"),
    ("adhoc-signed", "libfrust_probe_adhoc.dylib"),
    ("negative-control", "libfrust_probe_control.dylib"),
];
/// The value `frust_probe_value` returns.
const EXPECTED: u32 = 42;

/// One strategy's outcome: the value the patch returned, or the loader's error text.
pub struct ProbeOutcome {
    strategy: &'static str,
    outcome: Result<u32, String>,
}

impl ProbeOutcome {
    /// The console line `probe.sh` greps for.
    fn log_line(&self) -> String {
        match &self.outcome {
            Ok(value) => format!("frust-probe: strategy={} result={value}", self.strategy),
            Err(err) => format!("frust-probe: strategy={} error={err}", self.strategy),
        }
    }

    /// The on-screen summary for this strategy.
    fn label(&self) -> String {
        match &self.outcome {
            Ok(value) if *value == EXPECTED => {
                format!("{}: loaded, returned {value}", self.strategy)
            }
            Ok(value) => format!("{}: loaded, UNEXPECTED value {value}", self.strategy),
            Err(err) => format!("{}: FAILED: {err}", self.strategy),
        }
    }
}

/// Resolve `frust_probe_value` in `lib` and call it. The library is leaked on purpose: like
/// `frust_hotpatch::apply_patch`, the probe never unloads code it loaded, and the image must stay
/// mapped for `probe.sh`'s `vmmap`.
fn call_probe_value(lib: libloading::Library) -> Result<u32, String> {
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    // SAFETY: `patch/` exports `frust_probe_value` as `extern "C" fn() -> u32`.
    let probe = unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"frust_probe_value") }
        .map_err(|err| format!("dlsym frust_probe_value failed: {err}"))?;
    // SAFETY: the symbol has exactly this signature, takes no arguments and touches no state.
    Ok(unsafe { probe() })
}

/// The delivered file `name` inside the data dir, or the reason there is none to load.
fn patch_path(name: &str) -> Result<PathBuf, String> {
    let dir = frust_paths::data_dir().ok_or("frust-paths data_dir is not available")?;
    let path = dir.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("patch file missing: {}", path.display()))
    }
}

/// Load `path` with the frust-hotpatch platform loader (plain `dlopen` on iOS).
fn load(path: &Path) -> Result<u32, String> {
    // SAFETY: the file is the probe library built from `patch/` (no initialisers), or the
    // negative control, which the loader rejects before running anything.
    let lib = unsafe { frust_hotpatch::load_patch_library(path) }.map_err(|err| err.to_string())?;
    call_probe_value(lib)
}

/// Run one strategy, logging the attempt line before the load and the outcome line after it.
fn run_strategy(strategy: &'static str, file: &str) -> ProbeOutcome {
    let outcome = patch_path(file).and_then(|path| {
        log::info!(
            "frust-probe-attempt: strategy={strategy} pid={} path={}",
            std::process::id(),
            path.display()
        );
        load(&path)
    });
    let outcome = ProbeOutcome { strategy, outcome };
    log::info!("{}", outcome.log_line());
    outcome
}

/// Run the strategy `FRUST_PROBE_STRATEGY` names, or all of them in order when it is unset.
fn run_probe() -> Vec<ProbeOutcome> {
    let only = std::env::var("FRUST_PROBE_STRATEGY").ok();
    let selected: Vec<_> = STRATEGIES
        .iter()
        .filter(|(strategy, _)| only.as_deref().is_none_or(|name| name == *strategy))
        .collect();
    if selected.is_empty() {
        log::info!(
            "frust-probe: strategy={} error=unknown strategy",
            only.as_deref().unwrap_or_default()
        );
    }
    selected
        .into_iter()
        .map(|(strategy, file)| run_strategy(strategy, file))
        .collect()
}

/// The root component: runs the probe in `init` and shows each strategy's result.
#[derive(Default)]
pub struct ProbeApp;

impl Component for ProbeApp {
    type State = Vec<ProbeOutcome>;

    fn init(&self) -> Vec<ProbeOutcome> {
        run_probe()
    }

    fn build(&self, outcomes: &mut Vec<ProbeOutcome>) -> impl View<Vec<ProbeOutcome>> {
        safe_area(Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("frust iOS simulator patch-load probe").size(20.0))
                .children(
                    outcomes
                        .iter()
                        .map(|outcome| text(outcome.label()))
                        .collect::<Vec<_>>(),
                ),
        ))
    }
}

// The template's entry line, unchanged.
frust::app!(
    ProbeApp,
    setup = {
        frust_material::install();
    }
);
