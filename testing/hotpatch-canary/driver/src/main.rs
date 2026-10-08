//! The hot-patch CI canary's headless driver.
//!
//! `hotpatch-canary-driver <fixture-root>` fat-builds the fixture app with
//! the driver itself as cargo's rustc wrapper and rustc's linker stand-in
//! (the role the `frust` binary plays in a real session), launches the fat
//! image, then runs two edits against the live process:
//!
//! 1. the hot function's return value changes: thin build, L3 must pass,
//!    stub + thin link + jump table, applied in-process by the app, which
//!    must answer the new value through its `HotFn`;
//! 2. a field is added to the type the hot function returns (RESULTS.md row
//!    D2): thin build, L3 must answer `RestartRequired { LayoutChanged }`,
//!    and nothing is linked, sent or applied.
//!
//! Every surprise fails the run; the edited source is restored on exit.
//! Run it through `scripts/ci/hotpatch-canary.sh`.

mod app;
mod builder;
mod edit;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use frust_drive::hotpatch::capture;
use frust_drive::hotpatch::session::RestartReason;

use crate::app::{App, field};
use crate::builder::{FatSession, Toolchain};
use crate::edit::{D2_EDIT, D2_TYPE, EDITED_VALUE, SourceGuard, VALUE_EDIT};

/// The value the unedited hot function returns.
const BASE_VALUE: u64 = 1;

fn main() -> ExitCode {
    if let Some(scope_dir) = capture::wrapper_scope_from_env() {
        return run_wrapper(&scope_dir);
    }
    match run() {
        Ok(()) => {
            say("PASS");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("canary: FAIL: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// The driver as cargo's `RUSTC_WORKSPACE_WRAPPER` (and rustc's linker):
/// record or intercept this one invocation, as `frust-cli`'s `main` does.
fn run_wrapper(scope_dir: &Path) -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let envs: Vec<_> = std::env::vars_os().collect();
    let result = capture::run_wrapper(
        &frust_drive::process::RealProcessRunner,
        scope_dir,
        &args,
        &envs,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            let _ = writeln!(std::io::stderr(), "hotpatch-canary-driver: {err}");
            ExitCode::FAILURE
        }
    }
}

fn say(line: &str) {
    println!("canary: {line}");
}

fn run() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("usage: hotpatch-canary-driver <fixture-root>"))?;
    let root = std::fs::canonicalize(&root)
        .with_context(|| format!("resolving the fixture root `{}`", root.display()))?;
    let source = root.join("app").join("src").join("lib.rs");
    let driver_exe = capture::frust_exe()?;

    let toolchain = Toolchain::detect()?;
    say(&format!("toolchain: {}", toolchain.summary()));

    let mut session = FatSession::build(&root, &driver_exe, &toolchain, &mut |line| {
        println!("canary: {line}");
    })?;
    let base = session.accepted_layouts();
    let reading = base
        .get(D2_TYPE)
        .ok_or_else(|| anyhow!("the base layout table has no `{D2_TYPE}`; L3 would be blind"))?;
    say(&format!(
        "fat build: done; L3 base table: {} types (`{D2_TYPE}`: {} bytes), {} seam instances",
        base.len(),
        reading.size,
        session.accepted_seams().len()
    ));

    let (mut app, ready) = App::spawn(session.exe(), &root)?;
    say(&format!(
        "app ready: pid {} anchor {:#x} value {}",
        ready.pid, ready.anchor_runtime, ready.value
    ));
    if ready.value != BASE_VALUE {
        bail!(
            "the fat image answers value {}, expected {BASE_VALUE}",
            ready.value
        );
    }

    let mut guard = SourceGuard::new(&source)?;

    // Edit 1: the hot function's return value.
    guard.apply(&VALUE_EDIT)?;
    say(&format!(
        "edit 1: {} in `{}`",
        VALUE_EDIT.name,
        guard.path().display()
    ));
    let candidate = session.compile(guard.path())?;
    say(&format!(
        "thin build: replayed {:?}; candidate layouts: {} types",
        units(&candidate.replayed),
        candidate.layouts.len()
    ));
    let present = session
        .check(&candidate)
        .map_err(|reason| anyhow!("L3 refused a body-only edit: RestartRequired({reason:?})"))?;
    say("L3: pass (no accepted type changed layout)");
    let patch = session.link(1, ready.anchor_runtime)?;
    say(&format!(
        "thin build: linked `{}` ({} bytes, {} jump-table entries)",
        patch.path.display(),
        patch.bytes,
        patch.table.map.len()
    ));
    let table = serde_json::to_string(&patch.table).context("serializing the jump table")?;
    let answer = app.request(&table)?;
    say(&format!("app: {answer}"));
    if !answer.starts_with("applied ") {
        bail!("the app did not apply patch 1: `{answer}`");
    }
    expect_field(&answer, "value", &EDITED_VALUE.to_string())?;
    expect_field(&answer, "patches", "1")?;
    if field(&answer, "hits")?.parse::<u64>().unwrap_or(0) == 0 {
        bail!("the new value did not come through the jump table (no HotFn hit): `{answer}`");
    }
    session.accept(&candidate, &present)?;
    say(&format!(
        "applied: value {EDITED_VALUE} through the patched HotFn"
    ));

    // Edit 2: RESULTS.md row D2.
    guard.apply(&D2_EDIT)?;
    say(&format!("edit 2: {}", D2_EDIT.name));
    let candidate = session.compile(guard.path())?;
    say(&format!(
        "thin build: replayed {:?}; candidate layouts: {} types",
        units(&candidate.replayed),
        candidate.layouts.len()
    ));
    match session.check(&candidate) {
        Ok(_) => bail!("L3 passed the D2 edit; the patch would have been applied"),
        Err(RestartReason::LayoutChanged { records }) => {
            say(&format!(
                "RestartRequired {{ LayoutChanged }}: {}",
                records.join("; ")
            ));
            if !records.iter().any(|record| record.contains(D2_TYPE)) {
                bail!("the LayoutChanged records do not name `{D2_TYPE}`");
            }
        }
        Err(other) => bail!("the D2 edit was refused for the wrong reason: {other:?}"),
    }
    let answer = app.request("value")?;
    expect_field(&answer, "value", &EDITED_VALUE.to_string())?;
    expect_field(&answer, "patches", "1")?;
    say(&format!(
        "no apply: nothing linked or sent for the D2 edit; the app answers `{answer}`"
    ));

    app.finish()?;
    drop(guard);
    say(&format!("restored `{}`", source.display()));
    Ok(())
}

fn units(units: &[frust_drive::hotpatch::graph::ReplayUnit]) -> Vec<String> {
    units.iter().map(ToString::to_string).collect()
}

fn expect_field(answer: &str, key: &str, expected: &str) -> Result<()> {
    let got = field(answer, key)?;
    if got != expected {
        bail!("expected {key}={expected} in the app's answer, got `{answer}`");
    }
    Ok(())
}
