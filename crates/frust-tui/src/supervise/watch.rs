//! Per-session "Watch: restart on save" source watchers.
//!
//! A watched desktop session gets one [`SourceWatcher`]: a [`notify`] watcher
//! over `<project_root>/src` (recursive) and `<project_root>/Cargo.toml`
//! (non-recursive), feeding raw change ticks into a debounce thread that
//! collapses a save-burst into **one** [`Message::WatchTriggered`] for its
//! session. The pure engine decides what that message means (a restart
//! through the keyboard restart path, or nothing when the flag has since been
//! turned off); this module only notices changes.
//!
//! The debounce rule is `frust run --watch`'s (`frust-cli`'s
//! `watch_loop_with_slot`): a **trailing edge** — after the first tick, keep
//! consuming ticks that arrive within [`WATCH_DEBOUNCE`] of the previous one,
//! and act only once the tree has been quiet for that long. The loop is
//! duplicated here rather than shared: `frust-tui` does not depend on
//! `frust-cli`, and extracting it into `frust-drive` is a follow-up.
//!
//! Unlike the CLI loop this one filters events before ticking: reads
//! (inotify reports `IN_OPEN`, and every rebuild opens every source file)
//! would otherwise re-trigger the very restart that caused them, and editor
//! swap/backup files, hidden paths and build output directories are noise.
//!
//! [`SourceWatchers`] owns every live watcher, keyed by session id, next to
//! the runner's launch records. Stopping one hands back a [`Teardown`] (the
//! same bounded, detaching wait the devtools/metrics bridges use), and its
//! `Drop` stops them all against one deadline on quit, so no watcher thread
//! outlives the workbench.

use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use super::{SessionId, TEARDOWN_DEADLINE, Teardown, spawn_tracked};
use crate::engine::Message;

/// The trailing-edge debounce window — `frust run --watch`'s own figure, so a
/// save-burst restarts a watched TUI session exactly as often as it relaunches
/// the CLI loop.
pub const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// What the debounce thread receives: a raw filesystem change, or the stop
/// request [`SourceWatcher::stop`] sends. Stop is explicit rather than the
/// channel disconnecting because dropping a `notify` watcher releases its
/// event handler (and so its sender) asynchronously, on the backend's own
/// thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    /// Something under the watched paths changed.
    Change,
    /// Stop now, without acting on any burst still being debounced.
    Stop,
}

/// The testable core of the debounce thread: waits for a first
/// [`Signal::Change`], keeps consuming changes that arrive within `debounce`
/// of the previous one, then calls `fire` once — and loops. Returns on
/// [`Signal::Stop`] or a disconnected channel, **without** firing for a burst
/// that was still settling: a stopped watcher's session is being stopped,
/// restarted or closed, and a late restart request would only be refused.
fn debounce_loop(rx: &mpsc::Receiver<Signal>, debounce: Duration, mut fire: impl FnMut()) {
    loop {
        match rx.recv() {
            Ok(Signal::Change) => {}
            Ok(Signal::Stop) | Err(mpsc::RecvError) => return,
        }
        loop {
            match rx.recv_timeout(debounce) {
                Ok(Signal::Change) => {}
                Ok(Signal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
        fire();
    }
}

/// Whether a raw `notify` event is a source change worth a restart.
///
/// Reads never are — `IN_OPEN`/close-after-read, and an atime bump, are what a
/// rebuild itself produces. A write-close, create, remove, rename or content
/// change is, as long as at least one of its paths is relevant
/// ([`is_relevant_path`]); an event naming no path at all (a backend rescan)
/// is kept, since something may have changed.
fn is_relevant_event(root: &Path, event: &notify::Event) -> bool {
    let kind_counts = match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => false,
        EventKind::Any
        | EventKind::Create(_)
        | EventKind::Modify(_)
        | EventKind::Remove(_)
        | EventKind::Other => true,
    };
    kind_counts && (event.paths.is_empty() || event.paths.iter().any(|p| is_relevant_path(root, p)))
}

/// Whether `path` (as `notify` reports it, normally under `root`) is a source
/// path rather than noise: nothing under `<root>/target` or `<root>/build`,
/// no hidden component (`.git`, an editor's `.foo.rs.swp`), and no `~`-suffixed
/// editor backup.
fn is_relevant_path(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut components = rel.components().filter_map(|c| match c {
        Component::Normal(name) => Some(name.to_string_lossy()),
        _ => None,
    });
    let Some(first) = components.next() else {
        // The root itself: nothing to judge by name.
        return true;
    };
    if first == "target" || first == "build" {
        return false;
    }
    let mut last = first.clone();
    for name in std::iter::once(first).chain(components) {
        if name.starts_with('.') {
            return false;
        }
        last = name;
    }
    !last.ends_with('~')
}

/// One watched session's watcher: the `notify` handle plus the debounce
/// thread it feeds. Stop it with [`Self::stop`]; it is never dropped live
/// (every owner goes through [`SourceWatchers`]).
struct SourceWatcher {
    /// Keeps the OS-level watch alive; dropping it stops the raw events.
    watcher: notify::RecommendedWatcher,
    /// The debounce thread's inbox — also carries the stop request.
    signal: mpsc::Sender<Signal>,
    /// The debounce thread.
    worker: JoinHandle<()>,
    /// Disconnects when the debounce thread returns (see `spawn_tracked`).
    done: mpsc::Receiver<()>,
    /// Names the thread in a teardown's detach report.
    label: String,
}

impl SourceWatcher {
    /// Watch `root`'s `src/` tree and `Cargo.toml` for `session`, sending one
    /// [`Message::WatchTriggered`] per settled burst into `tx`.
    ///
    /// Either path missing is not an error (a project with no `src/` yet
    /// still gets a watcher over what exists), matching the CLI loop.
    fn start(
        session: SessionId,
        root: &Path,
        tx: UnboundedSender<Message>,
    ) -> Result<Self, String> {
        let (signal, inbox) = mpsc::channel::<Signal>();
        let raw = signal.clone();
        let filter_root = root.to_path_buf();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && is_relevant_event(&filter_root, &event)
            {
                // The debounce thread may already be gone (stopped); a
                // failed send is nobody's problem here.
                let _ = raw.send(Signal::Change);
            }
        })
        .map_err(|e| format!("creating the filesystem watcher failed: {e}"))?;
        let src = root.join("src");
        if src.is_dir() {
            watcher
                .watch(&src, RecursiveMode::Recursive)
                .map_err(|e| format!("watching `{}` failed: {e}", src.display()))?;
        }
        let cargo_toml = root.join("Cargo.toml");
        if cargo_toml.is_file() {
            watcher
                .watch(&cargo_toml, RecursiveMode::NonRecursive)
                .map_err(|e| format!("watching `{}` failed: {e}", cargo_toml.display()))?;
        }
        let label = format!("the source watcher for session {}", session.0);
        let (worker, done) = spawn_debouncer(session, WATCH_DEBOUNCE, inbox, move |msg| {
            let _ = tx.send(msg);
        })
        .map_err(|e| format!("spawning the watch thread failed: {e}"))?;
        Ok(Self {
            watcher,
            signal,
            worker,
            done,
            label,
        })
    }

    /// Stop watching: drop the OS watch, tell the debounce thread to return
    /// (abandoning any burst still settling), and hand its bounded wait back.
    fn stop(self) -> Teardown {
        let Self {
            watcher,
            signal,
            worker,
            done,
            label,
        } = self;
        drop(watcher);
        let _ = signal.send(Signal::Stop);
        Teardown::new(label, done, worker)
    }
}

/// Spawn the tracked debounce thread for `session` over `inbox`, delivering
/// each settled burst's [`Message::WatchTriggered`] to `deliver` — the seam
/// the tests drive with synthetic ticks and no filesystem.
fn spawn_debouncer(
    session: SessionId,
    debounce: Duration,
    inbox: mpsc::Receiver<Signal>,
    deliver: impl Fn(Message) + Send + 'static,
) -> std::io::Result<(JoinHandle<()>, mpsc::Receiver<()>)> {
    spawn_tracked(format!("frust-tui-watch-{}", session.0), move || {
        debounce_loop(&inbox, debounce, || {
            deliver(Message::WatchTriggered { session });
        });
    })
}

/// Every live [`SourceWatcher`], keyed by the session it watches for — owned
/// by the runner next to its launch records.
pub struct SourceWatchers {
    /// The engine channel each watcher's trigger is posted on.
    tx: UnboundedSender<Message>,
    /// The live watchers.
    live: HashMap<SessionId, SourceWatcher>,
}

impl SourceWatchers {
    /// An empty set posting into `tx`.
    pub fn new(tx: UnboundedSender<Message>) -> Self {
        Self {
            tx,
            live: HashMap::new(),
        }
    }

    /// Start watching `root` for `session`. A watcher already running for
    /// that session is replaced (its teardown is returned alongside), so a
    /// repeated start never leaves two threads triggering the same session.
    pub fn start(
        &mut self,
        session: SessionId,
        root: &Path,
    ) -> (Result<(), String>, Option<Teardown>) {
        let replaced = self.stop(session);
        let started = SourceWatcher::start(session, root, self.tx.clone()).map(|watcher| {
            self.live.insert(session, watcher);
        });
        (started, replaced)
    }

    /// Stop `session`'s watcher, if it has one.
    pub fn stop(&mut self, session: SessionId) -> Option<Teardown> {
        self.live.remove(&session).map(SourceWatcher::stop)
    }

    /// Whether `session` currently has a watcher.
    pub fn contains(&self, session: SessionId) -> bool {
        self.live.contains_key(&session)
    }

    /// Stop every watcher whose session `keep` rejects — the runner's
    /// reconciliation against the model, for a watched tab that went away
    /// without an effect naming it (a terminal tab closed on the spot).
    pub fn retain(&mut self, keep: impl Fn(SessionId) -> bool) -> Vec<Teardown> {
        let gone: Vec<SessionId> = self.live.keys().copied().filter(|id| !keep(*id)).collect();
        gone.into_iter().filter_map(|id| self.stop(id)).collect()
    }
}

impl Drop for SourceWatchers {
    fn drop(&mut self) {
        // Quit: signal every watcher first so they wind down in parallel,
        // then wait them all out against one shared deadline.
        let mut teardowns: Vec<Teardown> = self.live.drain().map(|(_, w)| w.stop()).collect();
        let deadline = Instant::now() + TEARDOWN_DEADLINE;
        for teardown in &mut teardowns {
            teardown.wait_until(deadline);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange};
    use std::path::PathBuf;
    use std::thread;

    /// Run [`debounce_loop`] on its own thread, reporting each fire on the
    /// returned receiver.
    fn spawn_counting(
        debounce: Duration,
    ) -> (
        mpsc::Sender<Signal>,
        mpsc::Receiver<()>,
        thread::JoinHandle<()>,
    ) {
        let (signal, inbox) = mpsc::channel();
        let (fired_tx, fired) = mpsc::channel();
        let worker = thread::spawn(move || {
            debounce_loop(&inbox, debounce, || {
                let _ = fired_tx.send(());
            });
        });
        (signal, fired, worker)
    }

    #[test]
    fn five_ticks_inside_the_window_collapse_into_one_trigger() {
        let (signal, fired, worker) = spawn_counting(WATCH_DEBOUNCE);
        for _ in 0..5 {
            signal.send(Signal::Change).unwrap();
        }
        fired
            .recv_timeout(WATCH_DEBOUNCE * 4)
            .expect("the settled burst fires once");
        assert!(
            fired
                .recv_timeout(WATCH_DEBOUNCE + WATCH_DEBOUNCE / 3)
                .is_err(),
            "one burst, one trigger"
        );
        signal.send(Signal::Stop).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn ticks_further_apart_than_the_window_trigger_twice() {
        let (signal, fired, worker) = spawn_counting(WATCH_DEBOUNCE);
        signal.send(Signal::Change).unwrap();
        thread::sleep(Duration::from_millis(400));
        signal.send(Signal::Change).unwrap();
        fired
            .recv_timeout(WATCH_DEBOUNCE * 4)
            .expect("first burst fires");
        fired
            .recv_timeout(WATCH_DEBOUNCE * 4)
            .expect("second burst fires");
        signal.send(Signal::Stop).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn stop_mid_burst_returns_without_firing() {
        let (signal, fired, worker) = spawn_counting(WATCH_DEBOUNCE);
        signal.send(Signal::Change).unwrap();
        signal.send(Signal::Stop).unwrap();
        worker.join().unwrap();
        assert!(fired.try_recv().is_err(), "a stopped burst never fires");
    }

    #[test]
    fn the_trigger_names_its_session_and_stop_joins_the_thread() {
        let (signal, inbox) = mpsc::channel();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (worker, done) = spawn_debouncer(SessionId(4), WATCH_DEBOUNCE, inbox, move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        signal.send(Signal::Change).unwrap();
        let deadline = Instant::now() + WATCH_DEBOUNCE * 4;
        let msg = loop {
            match rx.try_recv() {
                Ok(msg) => break msg,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("no trigger arrived: {e:?}"),
            }
        };
        assert_eq!(
            msg,
            Message::WatchTriggered {
                session: SessionId(4)
            }
        );

        signal.send(Signal::Stop).unwrap();
        let started = Instant::now();
        Teardown::new("the test debouncer".to_string(), done, worker).wait();
        assert!(
            started.elapsed() < TEARDOWN_DEADLINE,
            "a stopped debouncer is joined, not waited out"
        );
        // The thread returned, dropping its `deliver` closure (and so the
        // only sender): the channel is closed, not merely empty.
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn source_paths_count_and_noise_does_not() {
        let root = PathBuf::from("/p/app");
        assert!(is_relevant_path(&root, &root.join("src/main.rs")));
        assert!(is_relevant_path(&root, &root.join("src/ui/view.rs")));
        assert!(is_relevant_path(&root, &root.join("Cargo.toml")));
        assert!(!is_relevant_path(&root, &root.join("target/debug/app")));
        assert!(!is_relevant_path(&root, &root.join("build/out.o")));
        assert!(!is_relevant_path(&root, &root.join("src/.main.rs.swp")));
        assert!(!is_relevant_path(&root, &root.join(".git/index")));
        assert!(!is_relevant_path(&root, &root.join("src/main.rs~")));
        // A nested dir merely *named* `target` inside src/ is still source.
        assert!(is_relevant_path(&root, &root.join("src/target/mod.rs")));
    }

    #[test]
    fn reads_never_count_but_writes_do() {
        let root = PathBuf::from("/p/app");
        let src = root.join("src/main.rs");
        let event = |kind| notify::Event::new(kind).add_path(src.clone());
        assert!(!is_relevant_event(
            &root,
            &event(EventKind::Access(AccessKind::Open(AccessMode::Any)))
        ));
        assert!(!is_relevant_event(
            &root,
            &event(EventKind::Access(AccessKind::Close(AccessMode::Read)))
        ));
        assert!(!is_relevant_event(
            &root,
            &event(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::AccessTime
            )))
        ));
        assert!(is_relevant_event(
            &root,
            &event(EventKind::Access(AccessKind::Close(AccessMode::Write)))
        ));
        assert!(is_relevant_event(
            &root,
            &event(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
        ));
        assert!(is_relevant_event(
            &root,
            &event(EventKind::Create(CreateKind::File))
        ));
        // A write to a swap file is still noise.
        let swap = notify::Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(root.join("src/.main.rs.swp"));
        assert!(!is_relevant_event(&root, &swap));
    }
}
