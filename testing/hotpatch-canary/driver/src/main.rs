//! The hot-patch CI canary's headless driver.
//!
//! `hotpatch-canary-driver <fixture-root>` fat-builds the fixture app with
//! the driver itself as cargo's rustc wrappers and rustc's linker stand-in
//! (the role the `frust` binary plays in a real session), launches the fat
//! image, then runs four edits against the live process:
//!
//! 1. the hot function's return value changes: thin build, L3 must pass,
//!    stub + thin link + jump table, applied in-process by the app, which
//!    must answer the new value through its `HotFn`;
//! 2. the value the local path dependency `shared` (outside the workspace)
//!    returns changes: the thin build replays `shared` and the app lib, L3
//!    must pass, and the app must answer the sum through its `HotFn`;
//! 3. a field is added to the type the hot function returns (RESULTS.md row
//!    D2): thin build, L3 must answer `RestartRequired { LayoutChanged }`,
//!    and nothing is linked, sent or applied;
//! 4. a field is added to `shared`'s type the app's return type holds:
//!    likewise `RestartRequired { LayoutChanged }` naming it.
//!
//! After edit 1 is applied in-process, the same patch also goes through the
//! real devtools transport (the `wire` module): chunked patch and jump-table
//! uploads to a `frust-devtools` service in this driver, so a table over the
//! 1 MiB request line cap that the in-process apply never sees fails the run.
//!
//! Every surprise fails the run; the edited sources are restored on exit.
//! Thin-build and link times are printed per edit, and the script bounds them. Run it through
//! `scripts/ci/hotpatch-canary.sh`.

mod app;
mod builder;
mod edit;
mod wire;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use frust_drive::hotpatch::capture;
use frust_drive::hotpatch::session::RestartReason;

use crate::app::{App, field};
use crate::builder::{FatSession, SHARED_PACKAGE, Toolchain};
use crate::edit::{
    D2_EDIT, D2_TYPE, EDITED_VALUE, Edit, SHARED_D2_EDIT, SHARED_D2_TYPE, SHARED_EDITED_VALUE,
    SHARED_VALUE_EDIT, SourceGuard, VALUE_EDIT,
};
use crate::wire::WireService;

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
    let shared_source = root.join("shared").join("src").join("lib.rs");
    let driver_exe = capture::frust_exe()?;

    let toolchain = Toolchain::detect()?;
    say(&format!("toolchain: {}", toolchain.summary()));

    let starting = Instant::now();
    let mut session = FatSession::build(&root, &driver_exe, &toolchain, &mut |line| {
        println!("canary: {line}");
    })?;
    say(&format!(
        "timing: fat start (fat build, fat link, L3 base table, symbol cache): {} ms",
        starting.elapsed().as_millis()
    ));
    let base = session.accepted_layouts();
    let mut sizes = Vec::new();
    for ty in [D2_TYPE, SHARED_D2_TYPE] {
        let entry = base
            .get(ty)
            .ok_or_else(|| anyhow!("the base layout table has no `{ty}`; L3 would be blind"))?;
        sizes.push(format!("`{ty}`: {} bytes", entry.size));
    }
    say(&format!(
        "fat build: done; L3 base table: {} types ({}), {} seam instances",
        base.len(),
        sizes.join(", "),
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

    let wire = WireService::start()?;
    let mut guard = SourceGuard::new(&source)?;
    let mut shared_guard = SourceGuard::new(&shared_source)?;

    // Edit 1: the hot function's return value.
    guard.apply(&VALUE_EDIT)?;
    patch(
        &mut session,
        &mut app,
        ready.anchor_runtime,
        Applied {
            n: 1,
            edit: &VALUE_EDIT,
            file: guard.path(),
            value: EDITED_VALUE,
            replays: &[],
            wire: Some((&wire, ready.pid)),
        },
    )?;

    // Edit 2: the local path dependency's value, replayed with the app lib.
    shared_guard.apply(&SHARED_VALUE_EDIT)?;
    patch(
        &mut session,
        &mut app,
        ready.anchor_runtime,
        Applied {
            n: 2,
            edit: &SHARED_VALUE_EDIT,
            file: shared_guard.path(),
            value: SHARED_EDITED_VALUE,
            replays: &[SHARED_PACKAGE],
            wire: None,
        },
    )?;

    // Edit 3: RESULTS.md row D2.
    guard.apply(&D2_EDIT)?;
    refuse(
        &mut session,
        &mut app,
        3,
        &D2_EDIT,
        guard.path(),
        D2_TYPE,
        "",
    )?;

    // Edit 4: row D2's shape in the path dependency.
    shared_guard.apply(&SHARED_D2_EDIT)?;
    refuse(
        &mut session,
        &mut app,
        4,
        &SHARED_D2_EDIT,
        shared_guard.path(),
        SHARED_D2_TYPE,
        "shared ",
    )?;

    app.finish()?;
    wire.stop();
    drop(guard);
    drop(shared_guard);
    say(&format!(
        "restored `{}` and `{}`",
        source.display(),
        shared_source.display()
    ));
    Ok(())
}

/// A body-only edit the canary expects patched in.
struct Applied<'a> {
    /// The patch's number, and the app's patch count once it is applied.
    n: u32,
    edit: &'a Edit,
    file: &'a Path,
    /// The value the app must answer through its `HotFn` afterwards.
    value: u64,
    /// Packages besides the tip lib the thin build must replay.
    replays: &'a [&'a str],
    /// The devtools service to also send the patch through, and the app's pid.
    wire: Option<(&'a WireService, u32)>,
}

/// Thin-builds the edit already made to `expect.file`, gates it (L3 must
/// pass), links it, has the app apply it and accepts it.
fn patch(
    session: &mut FatSession,
    app: &mut App,
    anchor_runtime: u64,
    expect: Applied<'_>,
) -> Result<()> {
    let Applied {
        n,
        edit,
        file,
        value,
        replays,
        wire,
    } = expect;
    say(&format!("edit {n}: {} in `{}`", edit.name, file.display()));
    let started = Instant::now();
    let candidate = session.compile(file)?;
    let compiled = started.elapsed();
    say(&format!(
        "thin build: replayed {:?} in {} ms; candidate layouts: {} types",
        units(&candidate.replayed),
        compiled.as_millis(),
        candidate.layouts.len()
    ));
    for package in replays {
        if !candidate
            .replayed
            .iter()
            .any(|unit| unit.package == *package)
        {
            bail!("edit {n} did not replay `{package}`");
        }
    }
    let present = session
        .check(&candidate)
        .map_err(|reason| anyhow!("L3 refused a body-only edit: RestartRequired({reason:?})"))?;
    say("L3: pass (no accepted type changed layout)");
    let linking = Instant::now();
    let linked = session.link(n, anchor_runtime)?;
    let link_ms = linking.elapsed().as_millis();
    say(&format!(
        "thin build: linked `{}` ({} bytes, {} jump-table entries) in {link_ms} ms",
        linked.path.display(),
        linked.bytes,
        linked.table.map.len()
    ));
    say(&format!(
        "timing: edit {n}: compile {} ms, link {link_ms} ms, total {} ms",
        compiled.as_millis(),
        started.elapsed().as_millis()
    ));
    let table = serde_json::to_string(&linked.table).context("serializing the jump table")?;
    let answer = app.request(&table)?;
    say(&format!("app: {answer}"));
    if !answer.starts_with("applied ") {
        bail!("the app did not apply patch {n}: `{answer}`");
    }
    expect_field(&answer, "value", &value.to_string())?;
    expect_field(&answer, "patches", &n.to_string())?;
    if field(&answer, "hits")?.parse::<u64>().unwrap_or(0) == 0 {
        bail!("the new value did not come through the jump table (no HotFn hit): `{answer}`");
    }
    session.accept(&candidate, &present)?;
    say(&format!("applied: value {value} through the patched HotFn"));
    if let Some((service, pid)) = wire {
        let patch = std::fs::read(&linked.path)
            .with_context(|| format!("reading `{}`", linked.path.display()))?;
        let (entries, table_bytes) =
            service.send(u64::from(n), pid, anchor_runtime, patch, &linked.table)?;
        say(&format!(
            "wire: applied table_entries={entries} table_bytes={table_bytes}"
        ));
    }
    Ok(())
}

/// Thin-builds the layout-changing edit already made to `file`, asserts L3
/// answers `RestartRequired { LayoutChanged }` naming `changed`, and that
/// the app, sent nothing, still answers the last applied value.
fn refuse(
    session: &mut FatSession,
    app: &mut App,
    n: u32,
    edit: &Edit,
    file: &Path,
    changed: &str,
    label: &str,
) -> Result<()> {
    say(&format!("edit {n}: {} in `{}`", edit.name, file.display()));
    let started = Instant::now();
    let candidate = session.compile(file)?;
    say(&format!(
        "thin build: replayed {:?} in {} ms; candidate layouts: {} types",
        units(&candidate.replayed),
        started.elapsed().as_millis(),
        candidate.layouts.len()
    ));
    say(&format!(
        "timing: edit {n}: compile {} ms (refused)",
        started.elapsed().as_millis()
    ));
    match session.check(&candidate) {
        Ok(_) => bail!(
            "L3 passed the {}; the patch would have been applied",
            edit.name
        ),
        Err(RestartReason::LayoutChanged { records }) => {
            say(&format!(
                "{label}RestartRequired {{ LayoutChanged }}: {}",
                records.join("; ")
            ));
            if !records.iter().any(|record| record.contains(changed)) {
                bail!("the LayoutChanged records do not name `{changed}`");
            }
        }
        Err(other) => bail!(
            "the {} was refused for the wrong reason: {other:?}",
            edit.name
        ),
    }
    let answer = app.request("value")?;
    expect_field(&answer, "value", &SHARED_EDITED_VALUE.to_string())?;
    expect_field(&answer, "patches", "2")?;
    say(&format!(
        "{label}no apply: nothing linked or sent for edit {n}; the app answers `{answer}`"
    ));
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
