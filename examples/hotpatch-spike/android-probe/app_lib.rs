//! Android patch-load probe: the `src/lib.rs` that `probe.sh` drops into a throwaway
//! `frust create` app (see README.md beside this file).
//!
//! The root component's `init` runs once at startup, before the first frame, and tries to load a
//! code library delivered after install (`libfrust_probe_patch.so`, built from `patch/`) three
//! ways. Each strategy logs one line at info level, which the Android shell routes to logcat under
//! the `frust` tag:
//!
//! - `memfd`: `frust_hotpatch::load_patch_library` on the file in the app's files dir — on
//!   Android that copies it into a memfd and opens it with `android_dlopen_ext`.
//! - `plain-dlopen`: `libloading::Library::new` (plain `dlopen`) on the same files-dir path.
//! - `cache-dir`: the same file copied into the app's cache dir, then plain `dlopen`.
//!
//! A loaded library is resolved for `frust_probe_value` and the symbol is called; it returns 42.
//! The probe tests LOADING only: no jump table, no relocation against the running library.
//!
//! The files/cache dirs come from `frust-paths`, which the Android shell fills from
//! `Context.getFilesDir()`/`getCacheDir()` in `nativeInitPlatform`, before `nativeInit` builds the
//! root component — no package path is hard-coded here.

use std::path::{Path, PathBuf};

use frust::{Component, EdgeInsets, Padding, View, column, safe_area, text};

/// File name of the library `probe.sh` copies into the app's files dir.
const PATCH_FILE: &str = "libfrust_probe_patch.so";
/// The value `frust_probe_value` returns.
const EXPECTED: u32 = 42;

/// One strategy's outcome: the value the patch returned, or the loader's error text.
pub struct ProbeOutcome {
    strategy: &'static str,
    outcome: Result<u32, String>,
}

impl ProbeOutcome {
    /// The logcat line `probe.sh` greps for.
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
/// `frust_hotpatch::apply_patch`, the probe never unloads code it loaded.
fn call_probe_value(lib: libloading::Library) -> Result<u32, String> {
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    // SAFETY: `patch/` exports `frust_probe_value` as `extern "C" fn() -> u32`.
    let probe = unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"frust_probe_value") }
        .map_err(|err| format!("dlsym frust_probe_value failed: {err}"))?;
    // SAFETY: the symbol has exactly this signature, takes no arguments and touches no state.
    Ok(unsafe { probe() })
}

/// The patch file inside `dir`, or the reason there is none to load.
fn patch_in(dir: Option<PathBuf>, which: &str) -> Result<PathBuf, String> {
    let dir = dir.ok_or_else(|| format!("frust-paths {which} is not installed"))?;
    let path = dir.join(PATCH_FILE);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("patch file missing: {}", path.display()))
    }
}

/// Strategy `memfd`: the frust-hotpatch platform loader.
fn load_memfd(path: &Path) -> Result<u32, String> {
    // SAFETY: the file is the probe library built from `patch/`; it has no initialisers.
    let lib = unsafe { frust_hotpatch::load_patch_library(path) }.map_err(|err| err.to_string())?;
    call_probe_value(lib)
}

/// Strategies `plain-dlopen` and `cache-dir`: plain `dlopen` through libloading.
fn load_plain(path: &Path) -> Result<u32, String> {
    // SAFETY: as `load_memfd`.
    let lib = unsafe { libloading::Library::new(path) }.map_err(|err| err.to_string())?;
    call_probe_value(lib)
}

/// Copy the files-dir patch into the cache dir and return the copy's path.
fn copy_to_cache(source: &Path) -> Result<PathBuf, String> {
    let cache = frust_paths::cache_dir().ok_or("frust-paths cache_dir is not installed")?;
    let dest = cache.join(PATCH_FILE);
    std::fs::copy(source, &dest)
        .map_err(|err| format!("copy {} -> {}: {err}", source.display(), dest.display()))?;
    Ok(dest)
}

/// Run all three strategies once, logging one line per strategy.
fn run_probe() -> Vec<ProbeOutcome> {
    let files_patch = patch_in(frust_paths::data_dir(), "data_dir");
    let outcomes = vec![
        ProbeOutcome {
            strategy: "memfd",
            outcome: files_patch.clone().and_then(|path| load_memfd(&path)),
        },
        ProbeOutcome {
            strategy: "plain-dlopen",
            outcome: files_patch.clone().and_then(|path| load_plain(&path)),
        },
        ProbeOutcome {
            strategy: "cache-dir",
            outcome: files_patch
                .and_then(|path| copy_to_cache(&path))
                .and_then(|path| load_plain(&path)),
        },
    ];
    for outcome in &outcomes {
        log::info!("{}", outcome.log_line());
    }
    outcomes
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
                .child(text("frust Android patch-load probe").size(20.0))
                .children(outcomes.iter().map(|outcome| text(outcome.label()))),
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
