//! Interrupted-release-build gate: a `frust build apk --release` killed
//! mid-Gradle must leave **no** `android/.frust-signing.properties` behind.
//!
//! That file carries the keystore and key passwords in plaintext for the whole
//! Gradle invocation — minutes of wall-clock. Its cleanup used to be a `Drop`
//! guard alone, which runs on a normal return or unwind and nothing else, so a
//! Ctrl-C during the build left the passwords sitting in the project tree (and
//! in a project generated before the scaffold gitignored the file, sitting
//! there *committable*). `frust-drive::interrupt` now arms the path before the
//! file is created and deletes it from a SIGINT/SIGTERM/SIGHUP handler and a
//! panic hook. These tests hold that closed **against the real binary and a
//! real signal**, which is the only way to exercise a code path whose entire
//! subject is a process dying.
//!
//! Each test drives the compiled `frust` (`CARGO_BIN_EXE_frust`, the same
//! entry a user hits) against a **fake toolchain**: a `PATH` holding a
//! two-line `rustup`/`cargo`, a `JAVA_HOME` holding a `java` that prints a
//! version string, and — the point — an `android/gradlew` that just sleeps.
//! The sleep is a stand-in for the real multi-minute Gradle invocation, and it
//! makes the window this test needs both wide and free: no Android SDK, no
//! NDK, no Gradle download, no Rust cross-compile. Unlike `build_e2e` (which
//! asserts on real signed artifacts and is `#[ignore]`d for it) these run in
//! the ordinary `cargo test` gate, in about a second each.
//!
//! Unix-only: the whole subject is POSIX signal disposition.
//!
//! **Negative control.** Both tests fail (the file is left behind) if
//! `signing::write_resolved`'s `interrupt::scrub_on_signal` arm is removed —
//! verified by doing exactly that while implementing them. A test whose
//! subject is "cleanup happens on a path with no destructors" is worth nothing
//! unless it can observe the absence of that cleanup.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// How long to wait for the CLI to reach its Gradle invocation (i.e. for the
/// generated signing file to appear). Generous: this is preflight plus a few
/// file writes, but a loaded machine still has to schedule a fresh process.
const FILE_APPEARS_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait for the signalled process to actually die. The handler's
/// work is a couple of `unlink`s, so anything near this bound is a hang, not
/// slowness — which is itself the regression to catch (a handler that blocks
/// leaves a process a second Ctrl-C cannot kill).
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

const POLL: Duration = Duration::from_millis(20);

/// Exit status `frust-drive::interrupt` terminates with: 128 + SIGINT(2), the
/// shell convention. Asserting on it proves *our* handler ran rather than the
/// default disposition (which reports a signal, not an exit code).
const INTERRUPTED_EXIT_CODE: i32 = 130;

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "frust-cli-interrupt-e2e-{tag}-{}-{n}",
        std::process::id()
    ))
}

/// Writes an executable `#!/bin/sh` script at `path`.
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().expect("script has a parent")).expect("mkdir -p");
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("writing the script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

/// A Frust project whose Android build blocks: enough `frust.toml` and
/// `android/` for `frust build apk --release` to pass project detection and the
/// release-signing gate, with a `gradlew` that sleeps instead of building.
/// Returns the project root.
///
/// The keystore is a placeholder file — the gate checks that `storeFile` lands
/// on something that exists, not that it is a valid JKS, and no real signing
/// happens here because Gradle never runs.
fn blocking_project(tag: &str) -> PathBuf {
    let dir = unique_dir(tag);
    let android = dir.join("android");
    std::fs::create_dir_all(&android).expect("creating the project");
    std::fs::write(
        dir.join("frust.toml"),
        "[app]\nname = \"interrupt_e2e\"\norg = \"dev.f0x\"\n",
    )
    .expect("writing frust.toml");
    std::fs::write(android.join("upload.jks"), b"not-a-real-jks").expect("writing the keystore");
    std::fs::write(
        android.join("key.properties"),
        "storePassword=hunter2\nkeyPassword=hunter2\nkeyAlias=upload\nstoreFile=upload.jks\n",
    )
    .expect("writing key.properties");
    // The stand-in for a multi-minute Gradle invocation. Never expected to
    // finish: every test signals the process group long before this elapses.
    write_script(&android.join("gradlew"), "sleep 600");
    dir
}

/// A `PATH` directory and a `JAVA_HOME` satisfying `frust build`'s Android
/// preflight (`rustup target list --installed`, `cargo ndk --version`,
/// `$JAVA_HOME/bin/java -version`) without a real toolchain.
fn fake_toolchain(root: &Path) -> (PathBuf, PathBuf) {
    let bin = root.join("fake-bin");
    let java_home = root.join("fake-jdk");
    write_script(&bin.join("rustup"), "echo aarch64-linux-android");
    write_script(&bin.join("cargo"), "exit 0");
    // `java -version` writes to stderr by convention, and that is where the
    // preflight looks first.
    write_script(
        &java_home.join("bin/java"),
        "echo 'openjdk version \"17.0.9\" 2023-10-17' 1>&2",
    );
    (bin, java_home)
}

fn generated_signing_file(project: &Path) -> PathBuf {
    project.join("android/.frust-signing.properties")
}

/// Spawns `frust build apk --release` in `project` **in its own process
/// group**, so the test can signal the CLI and its `gradlew` child together —
/// exactly what a terminal Ctrl-C does to a foreground job, and what keeps the
/// sleeping `gradlew` from outliving the test.
fn spawn_release_build(project: &Path) -> Child {
    use std::os::unix::process::CommandExt;

    let (bin, java_home) = fake_toolchain(project);
    let path = match std::env::var_os("PATH") {
        Some(existing) => format!("{}:{}", bin.display(), existing.to_string_lossy()),
        None => bin.display().to_string(),
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_frust"));
    command
        .args([
            "build",
            "apk",
            "--release",
            "--target-platform",
            "android-arm64",
        ])
        .current_dir(project)
        .env("PATH", path)
        .env("JAVA_HOME", java_home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.process_group(0);
    command.spawn().expect("failed to spawn `frust build`")
}

/// Blocks until `path` exists, i.e. until the CLI has written the plaintext
/// signing file and handed control to `gradlew`. Fails loudly rather than
/// signalling a process that never reached the window under test.
fn wait_for(path: &Path, child: &mut Child) {
    let deadline = Instant::now() + FILE_APPEARS_TIMEOUT;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        if let Some(status) = child.try_wait().expect("polling the build") {
            panic!(
                "`frust build` exited ({status}) before writing `{}`; the fake toolchain no \
                 longer satisfies preflight or the signing gate",
                path.display()
            );
        }
        std::thread::sleep(POLL);
    }
    panic!("`{}` never appeared", path.display());
}

/// Sends `signal` to the child's whole process group, the way a terminal
/// delivers Ctrl-C to a foreground job. Goes through `sh`'s `kill` builtin so
/// the test needs no `libc` dependency of its own.
fn signal_group(child: &Child, signal: &str) {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -{signal} -{}", child.id()))
        .status()
        .expect("failed to spawn `kill`");
    assert!(status.success(), "`kill -{signal}` failed: {status}");
}

/// Waits for the signalled child to die, failing on a hang rather than blocking
/// the test run forever. Returns its exit code, or `None` if it was terminated
/// by a signal instead of exiting.
fn wait_for_exit(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("polling the build") {
            return status.code();
        }
        std::thread::sleep(POLL);
    }
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -{}", child.id()))
        .status();
    panic!("the signalled `frust build` never exited — a handler that hangs is unkillable");
}

/// The shared body: start a release build, wait until the plaintext signing
/// file exists, signal the group, and assert the file is gone once the process
/// is.
fn interrupt_mid_build_leaves_no_secret(tag: &str, signal: &str) -> Option<i32> {
    let project = blocking_project(tag);
    let generated = generated_signing_file(&project);
    let mut child = spawn_release_build(&project);

    wait_for(&generated, &mut child);
    // The window this whole feature is about: the passwords are on disk and
    // Gradle is running.
    let contents = std::fs::read_to_string(&generated).expect("reading the generated file");
    assert!(
        contents.contains("storePassword=hunter2"),
        "the file under test must really hold the plaintext password: {contents}"
    );

    signal_group(&child, signal);
    let code = wait_for_exit(&mut child);

    assert!(
        !generated.exists(),
        "`{}` survived a {signal}-interrupted build; it holds the keystore passwords in plaintext",
        generated.display()
    );
    let _ = std::fs::remove_dir_all(&project);
    code
}

/// Ctrl-C during the Gradle invocation. The file must be gone, and the exit
/// status must be the handler's 130 — proving cleanup ran rather than the
/// process taking the default disposition.
#[test]
fn a_ctrl_c_during_the_gradle_build_leaves_no_plaintext_passwords() {
    let code = interrupt_mid_build_leaves_no_secret("sigint", "INT");
    assert_eq!(
        code,
        Some(INTERRUPTED_EXIT_CODE),
        "an interrupted build must exit through the handler, not be killed by the default \
         SIGINT disposition"
    );
}

/// The same for SIGTERM, which is what a CI runner (and `kill` with no
/// argument) sends. It reaches the same handler through `ctrlc`'s `termination`
/// feature.
#[test]
fn a_sigterm_during_the_gradle_build_leaves_no_plaintext_passwords() {
    let code = interrupt_mid_build_leaves_no_secret("sigterm", "TERM");
    assert_eq!(
        code,
        Some(INTERRUPTED_EXIT_CODE),
        "SIGTERM must run the same cleanup handler; `ctrlc` cannot tell the handler which \
         signal fired, so the status is SIGINT's 130 for both"
    );
}

/// The no-regression direction: a build that is *not* interrupted still deletes
/// the file on the ordinary path. Here Gradle "fails" immediately, so the
/// pipeline bails — the error path `Drop` has always covered — and the arming
/// added for the signal case must not have broken it.
#[test]
fn an_uninterrupted_failing_build_still_deletes_the_generated_file() {
    let project = blocking_project("failing");
    // Replace the sleeping stand-in with one that fails at once.
    write_script(&project.join("android/gradlew"), "echo boom 1>&2\nexit 1");

    let output = spawn_release_build(&project)
        .wait_with_output()
        .expect("waiting for the build");
    assert!(!output.status.success(), "the fake gradlew must fail");
    assert!(
        !generated_signing_file(&project).exists(),
        "the generated signing file must not outlive a failed build"
    );
    let _ = std::fs::remove_dir_all(&project);
}
