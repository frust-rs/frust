//! Process-wide termination handling for the drive pipelines: **one** owner of
//! SIGINT/SIGTERM/SIGHUP, a registry of secret files to delete before the
//! process dies, and the exit status to die with.
//!
//! **Why this exists.** `android_build::signing` writes the four resolved
//! release-signing values — both passwords in plaintext — to
//! `android/.frust-signing.properties` and relies on `GeneratedProperties`'
//! `Drop` to delete it again. `Drop` runs on a normal return or unwind only,
//! and the file lives across the **whole Gradle invocation**: minutes of
//! wall-clock during which a Ctrl-C took the default disposition (process
//! terminated, no destructors) and left the passwords sitting in the project
//! tree. A panic did the same — the shipped `[profile.release]` sets `panic =
//! "abort"`, so a released `frust` never unwinds at all. This module makes the
//! "always deleted" claim true for those two routes as well.
//!
//! **The mechanism.** [`scrub_on_signal`] arms a path *before* the file it
//! names can exist and returns a registration whose `Drop` disarms it; the
//! signal handler and the panic hook both delete every armed path. Registering
//! a path rather than handing over an open file means the arm can precede the
//! `create`, so there is no instant at which the file exists unprotected.
//!
//! **One handler per process.** `ctrlc::set_handler` errors on a second call
//! anywhere in the process, so this module owns the single registration and
//! every drive path that wants a say goes through it — including the logcat
//! stream's "Ctrl-C means stop, exit 0", which is [`exit_code_on_signal`]
//! rather than a competing handler. (`ios_run` and `frust run --watch`'s
//! desktop loop still install their own; neither can be reached in the same
//! process as an Android release build, and if that ever changes the second
//! install fails loudly instead of silently dropping one of the two.)
//!
//! **SIGTERM and SIGHUP too** — CI runners send SIGTERM and a closed terminal
//! sends SIGHUP, and either one used to leave the file behind. `ctrlc`'s
//! `termination` feature routes all three to the same handler. It does not
//! tell the handler *which* signal fired, so the exit status is the single
//! [`INTERRUPTED_EXIT_CODE`] rather than a per-signal 128+N; this crate has no
//! `libc` dependency to restore the default disposition and re-raise with
//! (which is the only way to reproduce the exact `WIFSIGNALED` status a shell
//! reports), and 130-for-SIGTERM is a cosmetic difference next to leaking a
//! keystore password.
//!
//! **Ctrl-C stays Ctrl-C.** The handler scrubs and then exits — it never
//! swallows the signal, never leaves the process alive-but-unkillable, and
//! does not depend on any pipeline reaching a cancellation boundary. A second
//! Ctrl-C is not needed and cannot hang: `ctrlc` runs the handler on its own
//! dedicated thread (so this code is *not* in an async-signal context and may
//! lock, allocate, and touch the filesystem freely), and the handler's work is
//! a few `unlink`s before `exit`.
//!
//! **The handler races the pipeline it interrupts.** A terminal Ctrl-C (and a
//! CI runner's `kill`) reaches the whole process group, so the streamed child
//! — `./gradlew`, `adb logcat`, `xcrun` — dies of the same signal at the same
//! instant. Its closed stdout pipe then walks the pipeline down its ordinary
//! "the tool failed" path (`Output { success: false }` → `bail!` → the CLI's
//! `Err` arm → exit 1) on the main thread, while [`terminate`] scrubs and
//! exits 130 on `ctrlc`'s thread. Whichever reaches `exit` first decides the
//! status the shell reports, which made the interrupted-build exit code a coin
//! flip. [`defer_to_pending_signal`] resolves it in the signal's favour: a
//! caller that reaped a signal-killed child parks instead of reporting a
//! failure, letting the handler own the exit. Deferring rather than
//! synthesising 130 at the call site keeps one exit-status owner (the logcat
//! phase's [`exit_code_on_signal`] override still decides its own status).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
// The defer below is Unix-only — its whole trigger is a child's `WTERMSIG`,
// which Windows has no equivalent of — so its clock types are too.
#[cfg(unix)]
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// Exit status a signal-terminated `frust` reports unless a pipeline asked for
/// another one: 128 + SIGINT(2), what a shell reports for a Ctrl-C'd child.
const INTERRUPTED_EXIT_CODE: i32 = 130;

/// The status [`terminate`] exits with. Mutable because
/// `android_run`'s logcat phase documents Ctrl-C as the *intended* way to stop
/// a streamed run and reports success for it — see [`exit_code_on_signal`].
static EXIT_CODE: AtomicI32 = AtomicI32::new(INTERRUPTED_EXIT_CODE);

/// Set by [`terminate`] before it does anything else: a one-way latch meaning
/// "a termination signal reached this process and the handler now owns how it
/// dies". One-way because every path that sets it ends in `process::exit` —
/// nothing clears it, and nothing may treat it as "a signal happened once".
static SIGNAL_RECEIVED: AtomicBool = AtomicBool::new(false);

/// How long [`defer_to_pending_signal`] waits for the handler to claim a
/// signal that has visibly already killed a child process. This bounds a
/// scheduling delay, not work: `ctrlc`'s handler thread is woken by a
/// semaphore the C-level handler posts, so it normally latches
/// [`SIGNAL_RECEIVED`] within microseconds of the child dying. The half-second
/// is slack for a loaded machine — generous because the only cost of waiting
/// too long is a slower exit on a process that is already terminating, while
/// waiting too little brings back the coin-flip exit status.
#[cfg(unix)]
pub(crate) const SIGNAL_DEFER_TIMEOUT: Duration = Duration::from_millis(500);

/// Poll cadence for [`defer_to_pending_signal`] — short enough that the common
/// case (the handler is already running) costs one sleep, coarse enough not to
/// spin against the very thread it is waiting for.
#[cfg(unix)]
const SIGNAL_DEFER_POLL: Duration = Duration::from_millis(5);

/// The process-wide armed set. `const`-constructed so no lazy initialisation
/// stands between a caller and arming a path.
static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());

/// The one-time install of the signal handler and panic hook, holding its
/// outcome so a second caller gets the same verdict instead of a spurious
/// `MultipleHandlers` error from `ctrlc`.
static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

/// The set of paths to delete when this process is about to die: secret files
/// whose `Drop`-based cleanup a signal or an abort would otherwise skip.
///
/// Kept as a plain value type (not a bag of statics) so its arm/disarm/scrub
/// behaviour is unit-testable against a local instance — a test that drove the
/// process-wide [`REGISTRY`] would race every other test's live secret file.
struct Registry {
    /// `(id, path)` pairs. The id is what a [`SecretScrub`] disarms by, so two
    /// registrations of the same path stay independent.
    entries: Vec<(u64, PathBuf)>,
    next_id: u64,
}

impl Registry {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 0,
        }
    }

    /// Arms `path`, returning the id that disarms it. The path need not exist
    /// yet — that is the point: the caller arms first and creates second.
    fn arm(&mut self, path: &Path) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push((id, path.to_path_buf()));
        id
    }

    fn disarm(&mut self, id: u64) {
        self.entries.retain(|(entry, _)| *entry != id);
    }

    /// Deletes every armed path, ignoring every failure: this runs while the
    /// process is already dying, an armed path that was never created (or was
    /// already deleted by its guard) is the normal case, and there is nobody
    /// left to report an error to.
    fn scrub(&self) {
        for (_, path) in &self.entries {
            let _ = fs::remove_file(path);
        }
    }

    #[cfg(test)]
    fn armed_paths(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|(_, path)| path.clone()).collect()
    }
}

/// The process-wide registry. A poisoned lock is recovered rather than
/// propagated: the data behind it is a list of paths that some *other* thread's
/// panic cannot have invalidated, and refusing to scrub because of that panic
/// would be exactly backwards.
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A live registration: while it is alive, a termination signal or a panic
/// deletes the path it names. Dropping it disarms that path — which is why the
/// owner of the file drops this only *after* removing the file itself.
#[must_use = "dropping the registration disarms the signal-time cleanup"]
pub(crate) struct SecretScrub {
    id: u64,
}

impl Drop for SecretScrub {
    fn drop(&mut self) {
        registry().disarm(self.id);
    }
}

/// Arms `path` for deletion on SIGINT/SIGTERM/SIGHUP or a panic, installing the
/// process-wide handler on first use.
///
/// Call this **before** creating the file, and hold the returned registration
/// until after it has been deleted; the path is allowed not to exist for as
/// long as the registration lives, so there is no window where a secret file
/// exists unarmed.
///
/// Fails only when the handler cannot be installed (another `ctrlc` handler
/// already owns SIGINT in this process) — a hard error rather than a silent
/// downgrade, since the caller's whole reason to arm is that it is about to
/// write a secret to disk.
pub(crate) fn scrub_on_signal(path: &Path) -> Result<SecretScrub> {
    install()?;
    let id = registry().arm(path);
    Ok(SecretScrub { id })
}

/// Sets the status [`terminate`] exits with, installing the process-wide
/// handler on first use.
///
/// This is how a pipeline that treats Ctrl-C as a normal end-of-session — the
/// Android logcat stream, whose "press Ctrl-C to stop" is the documented way
/// out — keeps reporting success, without installing a second handler that
/// would either fail to install or silently replace the secret-file scrub.
pub(crate) fn exit_code_on_signal(code: i32) -> Result<()> {
    install()?;
    EXIT_CODE.store(code, Ordering::SeqCst);
    Ok(())
}

/// The armed paths, for tests that need to assert on the process-wide state
/// (`signing`'s guard-lifecycle test) rather than on a local [`Registry`].
#[cfg(test)]
pub(crate) fn armed_paths() -> Vec<PathBuf> {
    registry().armed_paths()
}

/// What every termination signal ends in: scrub, then die. Runs on `ctrlc`'s
/// dedicated handler thread, so blocking on the registry lock is safe (the
/// thread holding it is making progress, and only ever holds it for a `push`,
/// a `retain`, or the scrub itself).
///
/// [`SIGNAL_RECEIVED`] is latched **first**, ahead of the scrub: a pipeline
/// thread that just reaped a child killed by the same group signal is polling
/// for it, and every instant it isn't set is an instant that thread may
/// instead report an ordinary tool failure and exit 1 out from under this one.
fn terminate() -> ! {
    SIGNAL_RECEIVED.store(true, Ordering::SeqCst);
    registry().scrub();
    std::process::exit(EXIT_CODE.load(Ordering::SeqCst))
}

/// Yields the process's exit status to a termination signal that is already in
/// flight, for a caller holding evidence of one: a child it just reaped was
/// killed by SIGINT/SIGTERM/SIGHUP, which for a process-group signal (a
/// terminal Ctrl-C, a CI runner's `kill`) means this process was signalled
/// too.
///
/// Parks **forever** once [`SIGNAL_RECEIVED`] is latched — the handler thread
/// is by then inside [`terminate`], whose `process::exit` ends every thread
/// including this one. The park is what stops the caller from racing ahead to
/// report the dead child as an ordinary tool failure and exiting 1 first.
///
/// Returns instead — after at most `timeout` — when no signal is pending here,
/// because a child can be signal-killed without this process being a target
/// (someone `kill`s a stuck `./gradlew` by pid, a supervisor reaps a subtree).
/// Reporting that child's failure normally is then the correct outcome, and
/// blocking on a signal that is never coming would hang the CLI outright —
/// hence a bounded wait rather than an unconditional park.
///
/// **Unix-only**, like the evidence it takes: only a Unix exit status carries
/// the terminating signal a caller reads to decide it should defer at all.
#[cfg(unix)]
pub(crate) fn defer_to_pending_signal(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if SIGNAL_RECEIVED.load(Ordering::SeqCst) {
            // The handler owns the exit from here. `park` can wake
            // spuriously, so this is a loop, not a single call.
            loop {
                std::thread::park();
            }
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(SIGNAL_DEFER_POLL);
    }
}

/// Installs the signal handler and panic hook exactly once per process,
/// returning the same verdict to every later caller.
fn install() -> Result<()> {
    match INSTALLED.get_or_init(install_once) {
        Ok(()) => Ok(()),
        Err(err) => bail!(
            "failed to install the interrupt handler that deletes \
             `android/.frust-signing.properties` if this build is interrupted: {err}\n\n\
             `frust` installs exactly one termination handler per process and something else \
             already owns SIGINT here. Refusing to write plaintext keystore passwords that a \
             Ctrl-C would then leave behind."
        ),
    }
}

fn install_once() -> Result<(), String> {
    // A release `frust` is built with `panic = "abort"`, so a panic runs no
    // destructor at all — but it does run this hook first. Chained onto the
    // previous hook rather than replacing it, so the usual panic message still
    // reaches stderr.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        scrub_from_panic_hook();
        previous(info);
    }));

    ctrlc::set_handler(|| terminate()).map_err(|err| err.to_string())
}

/// The panic-hook scrub. Unlike [`terminate`] this runs on the *panicking*
/// thread, so a blocking lock would deadlock outright if that thread panicked
/// while holding the registry — a shape no code here has, but a hang is a worse
/// failure than a missed scrub, so the hook never blocks. A poisoned lock is
/// still scrubbed (an earlier panic must not disarm a later one's cleanup).
fn scrub_from_panic_hook() {
    match REGISTRY.try_lock() {
        Ok(registry) => registry.scrub(),
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().scrub(),
        Err(TryLockError::WouldBlock) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn unique_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "frust-drive-interrupt-test-{tag}-{}-{n}",
            std::process::id()
        ))
    }

    fn touch(path: &Path) {
        fs::write(path, b"storePassword=hunter2\n").unwrap();
    }

    /// The whole point: what the signal handler runs deletes a file whose
    /// `Drop` guard never got the chance to.
    #[test]
    fn the_scrub_deletes_every_armed_path() {
        let first = unique_path("scrub-first");
        let second = unique_path("scrub-second");
        touch(&first);
        touch(&second);

        let mut registry = Registry::new();
        registry.arm(&first);
        registry.arm(&second);
        registry.scrub();

        assert!(!first.exists(), "an armed secret must not survive a signal");
        assert!(
            !second.exists(),
            "every armed path is scrubbed, not just one"
        );
    }

    /// Arming precedes creation by design — the registry holds a path, not a
    /// file handle — so an armed path that does not exist yet is normal and a
    /// scrub over it is a no-op rather than an error.
    #[test]
    fn a_path_can_be_armed_before_it_exists() {
        let path = unique_path("armed-early");
        let mut registry = Registry::new();
        registry.arm(&path);
        assert_eq!(registry.armed_paths(), vec![path.clone()]);

        registry.scrub();
        assert!(!path.exists());

        // …and the same registration still covers the file once it is created.
        touch(&path);
        registry.scrub();
        assert!(!path.exists(), "the arm covers the file it later names");
    }

    /// Disarming removes exactly one registration: a scrub afterwards leaves
    /// that path alone and still deletes the others.
    #[test]
    fn disarming_stops_the_scrub_for_that_path_only() {
        let kept = unique_path("disarm-kept");
        let scrubbed = unique_path("disarm-scrubbed");
        touch(&kept);
        touch(&scrubbed);

        let mut registry = Registry::new();
        let kept_id = registry.arm(&kept);
        registry.arm(&scrubbed);
        registry.disarm(kept_id);
        assert_eq!(registry.armed_paths(), vec![scrubbed.clone()]);

        registry.scrub();
        assert!(kept.exists(), "a disarmed path must be left alone");
        assert!(!scrubbed.exists());

        let _ = fs::remove_file(&kept);
    }

    /// Two registrations of the *same* path are independent — disarming one
    /// must not silently disarm the other (ids, not paths, are the key).
    #[test]
    fn two_registrations_of_one_path_are_independent() {
        let path = unique_path("double-arm");
        let mut registry = Registry::new();
        let first = registry.arm(&path);
        registry.arm(&path);
        registry.disarm(first);
        assert_eq!(registry.armed_paths(), vec![path]);
    }

    /// A phase that treats Ctrl-C as the documented way out (the logcat
    /// stream) sets the exit status through this module rather than installing
    /// a second handler that would replace the secret-file scrub.
    #[test]
    fn the_exit_status_is_settable_for_a_phase_that_reports_success() {
        let previous = EXIT_CODE.load(Ordering::SeqCst);
        exit_code_on_signal(0).unwrap();
        assert_eq!(EXIT_CODE.load(Ordering::SeqCst), 0);
        EXIT_CODE.store(previous, Ordering::SeqCst);
    }

    /// The bounded half of the defer contract. A child can be signal-killed
    /// without this process being a target, and in a test process nothing ever
    /// signals us — so the deferring caller must get control back after its
    /// timeout rather than parking on a signal that is never coming. (The
    /// unbounded half, parking once the latch is set, is not unit-testable:
    /// setting the latch for real ends in `process::exit`.)
    #[cfg(unix)]
    #[test]
    fn deferring_returns_when_no_signal_is_pending() {
        assert!(
            !SIGNAL_RECEIVED.load(Ordering::SeqCst),
            "a latched signal means the handler already ran, which exits the process"
        );
        let timeout = Duration::from_millis(30);
        let started = Instant::now();
        defer_to_pending_signal(timeout);
        assert!(
            started.elapsed() >= timeout,
            "the wait must be the full timeout, not an immediate return"
        );
    }

    /// The process-wide seam: arming registers, dropping the registration
    /// disarms. (The handler install this performs is idempotent, so every
    /// other test in the crate that writes a generated signing file shares it.)
    #[test]
    fn the_process_wide_registration_arms_and_disarms() {
        let path = unique_path("global");
        let guard = scrub_on_signal(&path).unwrap();
        assert!(armed_paths().contains(&path), "arming must be visible");
        drop(guard);
        assert!(
            !armed_paths().contains(&path),
            "dropping the registration must disarm it"
        );
    }
}
