//! Per-session "Watch: hot patch on save" source watchers.
//!
//! A watched desktop session gets one [`SourceWatcher`]: a [`notify`] watcher
//! feeding raw change events into a debounce thread that collapses a
//! save-burst into **one** [`Message::WatchTriggered`] for its session,
//! carrying every path the burst touched. The pure engine decides what that
//! message means (a hot patch for a hot session, a restart through the
//! keyboard restart path for any other, or nothing when the flag has since
//! been turned off); this module only notices changes.
//!
//! **What is watched.** Every watcher starts on `<project_root>/src`
//! (recursive) and `<project_root>/Cargo.toml` (non-recursive) — all a cold
//! (restart-on-save) session watches. A watcher for a **hot** session then
//! widens, on its own thread, to the hot watch set `frust run --watch`
//! registers too — `frust-drive`'s [`WatchSet`], converted by [`hot_scope`]:
//! the whole `src/` tree of every workspace member (replayable) and of every
//! local non-member path package, the directory of every lib/bin target root
//! outside `src/`, and the build inputs — every package's `Cargo.toml` and
//! `build.rs`, and the workspace and project roots' `Cargo.toml`,
//! `Cargo.lock`, `frust.toml`, `rust-toolchain`, `rust-toolchain.toml` and
//! `.cargo/config(.toml)`. A local non-member source replays when its
//! compile was captured (desktop) and restarts the session otherwise; the
//! session classifies, this watcher only names. The widening runs
//! `cargo metadata` (`WorkspaceGraph::load`), so it never touches the UI
//! thread, and a project it cannot resolve keeps the base scope. The hot
//! session's own graph is not reachable from here (`HotSession` exposes
//! none), so the watcher loads its own.
//!
//! The debounce rule is `frust run --watch`'s, and so is its window: one
//! [`WATCH_DEBOUNCE`], defined in `frust-drive` and re-exported here. It is a
//! **trailing edge** — after the first tick, keep consuming ticks that arrive
//! within the window of the previous one, and act only once the tree has been
//! quiet for that long. The loop itself stays this module's own.
//!
//! Unlike the CLI loop this one filters events before ticking: reads
//! (inotify reports `IN_OPEN`, and every rebuild opens every source file)
//! would otherwise re-trigger the very restart that caused them, and editor
//! swap/backup files, hidden paths (bar `.cargo/config`) and build output
//! directories are noise.
//!
//! [`SourceWatchers`] owns every live watcher, keyed by session id, next to
//! the runner's launch records. Stopping one hands back a [`Teardown`] (the
//! same bounded, detaching wait the devtools/metrics bridges use), and its
//! `Drop` stops them all against one deadline on quit, so no watcher thread
//! outlives the workbench.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use frust_drive::hotpatch::graph::WorkspaceGraph;
use frust_drive::hotpatch::watch::{WatchSet, is_relevant, watch_set_in};
use frust_drive::process::RealProcessRunner;
use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use super::{SessionId, TEARDOWN_DEADLINE, Teardown, spawn_tracked};
use crate::engine::Message;

/// The shared save debounce window (`frust-drive`'s, the one `frust run
/// --watch` uses), re-exported for the runner.
pub use frust_drive::hotpatch::watch::WATCH_DEBOUNCE;

/// What the debounce thread receives: a raw filesystem change, or the stop
/// request [`SourceWatcher::stop`] sends. Stop is explicit rather than the
/// channel disconnecting because dropping a `notify` watcher releases its
/// event handler (and so its sender) asynchronously, on the backend's own
/// thread.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Signal {
    /// Something under the watched paths changed: the relevant paths the
    /// event named (empty for a backend rescan that names none).
    Change(Vec<PathBuf>),
    /// Stop now, without acting on any burst still being debounced.
    Stop,
}

/// The testable core of the debounce thread: waits for a first
/// [`Signal::Change`], keeps consuming changes that arrive within `debounce`
/// of the previous one, then calls `fire` once with every path the burst
/// named (deduplicated, sorted) — and loops. Returns on [`Signal::Stop`] or
/// a disconnected channel, **without** firing for a burst that was still
/// settling: a stopped watcher's session is being stopped, restarted or
/// closed, and a late trigger would only be refused.
fn debounce_loop(
    rx: &mpsc::Receiver<Signal>,
    debounce: Duration,
    mut fire: impl FnMut(Vec<PathBuf>),
) {
    loop {
        let mut burst = BTreeSet::new();
        match rx.recv() {
            Ok(Signal::Change(paths)) => burst.extend(paths),
            Ok(Signal::Stop) | Err(mpsc::RecvError) => return,
        }
        loop {
            match rx.recv_timeout(debounce) {
                Ok(Signal::Change(paths)) => burst.extend(paths),
                Ok(Signal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
            }
        }
        fire(burst.into_iter().collect());
    }
}

/// The relevant paths of a raw `notify` event, or `None` when it is not a
/// source change worth a trigger.
///
/// Reads never are — `IN_OPEN`/close-after-read, and an atime bump, are what a
/// rebuild itself produces. A write-close, create, remove, rename or content
/// change is, as long as at least one of its paths is relevant
/// ([`is_relevant_path`]); an event naming no path at all (a backend rescan)
/// is kept with no paths, since something may have changed.
fn relevant_paths(roots: &[PathBuf], event: &notify::Event) -> Option<Vec<PathBuf>> {
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
    if !kind_counts {
        return None;
    }
    if event.paths.is_empty() {
        return Some(Vec::new());
    }
    let paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|p| is_relevant_path(roots, p))
        .cloned()
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// Whether `path` (as `notify` reports it, normally under one of `roots`) is
/// a source path rather than noise: `frust-drive`'s one relevance rule
/// ([`is_relevant`]).
fn is_relevant_path(roots: &[PathBuf], path: &Path) -> bool {
    is_relevant(roots, path)
}

/// What one watcher covers: each path and whether its subtree counts.
type WatchScope = Vec<(PathBuf, RecursiveMode)>;

/// The scope every watcher starts with — `frust run --watch`'s: the
/// project's `src/` tree and its `Cargo.toml`.
fn base_scope(root: &Path) -> WatchScope {
    vec![
        (root.join("src"), RecursiveMode::Recursive),
        (root.join("Cargo.toml"), RecursiveMode::NonRecursive),
    ]
}

/// A hot session's watch scope: the shared [`WatchSet`] as watcher entries —
/// its `src/` trees ([`WatchSet::dirs`]) recursively, its build inputs one
/// file each. No package directory (and so no `target/`) is ever watched
/// whole. Paths are returned whether or not they exist; the caller skips the
/// missing ones.
fn hot_scope(set: &WatchSet) -> WatchScope {
    set.dirs()
        .map(|dir| (dir.clone(), RecursiveMode::Recursive))
        .chain(
            set.build_inputs
                .iter()
                .map(|file| (file.clone(), RecursiveMode::NonRecursive)),
        )
        .collect()
}

/// Resolve `root`'s workspace graph (`cargo metadata`, blocking) into its
/// shared [`WatchSet`], as its [`hot_scope`] plus the roots a changed path is
/// judged relative to (the workspace root and every package directory).
/// `None` when the project has no `[package]` or cargo cannot describe it —
/// the watcher then keeps its base scope.
fn resolve_hot_scope(root: &Path) -> Option<(WatchScope, Vec<PathBuf>)> {
    let package = super::session::package_name(root)?;
    let graph = WorkspaceGraph::load(
        &RealProcessRunner,
        &root.join("Cargo.toml"),
        None,
        &package,
        None,
    )
    .ok()?;
    let set = watch_set_in(&graph, root);
    Some((hot_scope(&set), set.roots))
}

/// Watch every path of `scope` that exists, best effort: a path that fails
/// to watch is skipped, the rest still count.
fn watch_scope(watcher: &Mutex<notify::RecommendedWatcher>, scope: &[(PathBuf, RecursiveMode)]) {
    let mut watcher = watcher.lock().unwrap_or_else(|p| p.into_inner());
    for (path, mode) in scope {
        if path.exists() {
            let _ = watcher.watch(path, *mode);
        }
    }
}

/// One watched session's watcher: the `notify` handle plus the debounce
/// thread it feeds. Stop it with [`Self::stop`]; it is never dropped live
/// (every owner goes through [`SourceWatchers`]).
struct SourceWatcher {
    /// Keeps the OS-level watch alive; dropping the last handle stops the
    /// raw events. Shared with the debounce thread, which widens a hot
    /// watcher's scope before it starts debouncing.
    watcher: Arc<Mutex<notify::RecommendedWatcher>>,
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
    /// Watch `root`'s base scope ([`base_scope`]) for `session` — widened to
    /// the hot path classes on the debounce thread when `hot` — sending one
    /// [`Message::WatchTriggered`] per settled burst into `tx`.
    ///
    /// A missing base path is not an error (a project with no `src/` yet
    /// still gets a watcher over what exists), matching the CLI loop.
    fn start(
        session: SessionId,
        root: &Path,
        hot: bool,
        tx: UnboundedSender<Message>,
    ) -> Result<Self, String> {
        let (signal, inbox) = mpsc::channel::<Signal>();
        let raw = signal.clone();
        let roots = Arc::new(RwLock::new(vec![root.to_path_buf()]));
        let filter_roots = Arc::clone(&roots);
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            let roots = filter_roots.read().unwrap_or_else(|p| p.into_inner());
            if let Some(paths) = relevant_paths(&roots, &event) {
                // The debounce thread may already be gone (stopped); a
                // failed send is nobody's problem here.
                let _ = raw.send(Signal::Change(paths));
            }
        })
        .map_err(|e| format!("creating the filesystem watcher failed: {e}"))?;
        let watcher = Arc::new(Mutex::new(watcher));
        {
            let mut handle = watcher.lock().unwrap_or_else(|p| p.into_inner());
            for (path, mode) in base_scope(root) {
                if path.exists() {
                    handle
                        .watch(&path, mode)
                        .map_err(|e| format!("watching `{}` failed: {e}", path.display()))?;
                }
            }
        }
        let widen = {
            let watcher = Arc::clone(&watcher);
            let root = root.to_path_buf();
            move || {
                if !hot {
                    return;
                }
                if let Some((scope, extra_roots)) = resolve_hot_scope(&root) {
                    roots
                        .write()
                        .unwrap_or_else(|p| p.into_inner())
                        .extend(extra_roots);
                    watch_scope(&watcher, &scope);
                }
            }
        };
        let label = format!("the source watcher for session {}", session.0);
        let (worker, done) =
            spawn_debouncer(session, hot, WATCH_DEBOUNCE, inbox, widen, move |msg| {
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

/// Spawn the tracked debounce thread for `session` over `inbox`: it runs
/// `before` first (a hot watcher's scope widening), then delivers each
/// settled burst's [`Message::WatchTriggered`] to `deliver` — the seam the
/// tests drive with synthetic ticks and no filesystem.
fn spawn_debouncer(
    session: SessionId,
    hot: bool,
    debounce: Duration,
    inbox: mpsc::Receiver<Signal>,
    before: impl FnOnce() + Send + 'static,
    deliver: impl Fn(Message) + Send + 'static,
) -> std::io::Result<(JoinHandle<()>, mpsc::Receiver<()>)> {
    spawn_tracked(format!("frust-tui-watch-{}", session.0), move || {
        before();
        debounce_loop(&inbox, debounce, |paths| {
            deliver(Message::WatchTriggered {
                session,
                paths,
                hot,
            });
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

    /// Start watching `root` for `session`; `hot` says the session runs as a
    /// hot-patch session (its triggers say so, and its scope widens to the
    /// hot path classes). A watcher already running for that session is
    /// replaced (its teardown is returned alongside), so a repeated start
    /// never leaves two threads triggering the same session.
    pub fn start(
        &mut self,
        session: SessionId,
        root: &Path,
        hot: bool,
    ) -> (Result<(), String>, Option<Teardown>) {
        let replaced = self.stop(session);
        let started = SourceWatcher::start(session, root, hot, self.tx.clone()).map(|watcher| {
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
            debounce_loop(&inbox, debounce, |_| {
                let _ = fired_tx.send(());
            });
        });
        (signal, fired, worker)
    }

    #[test]
    fn five_ticks_inside_the_window_collapse_into_one_trigger() {
        let (signal, fired, worker) = spawn_counting(WATCH_DEBOUNCE);
        for _ in 0..5 {
            signal.send(change("/p/app/src/main.rs")).unwrap();
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
        signal.send(change("/p/app/src/main.rs")).unwrap();
        thread::sleep(Duration::from_millis(400));
        signal.send(change("/p/app/src/main.rs")).unwrap();
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
        signal.send(change("/p/app/src/main.rs")).unwrap();
        signal.send(Signal::Stop).unwrap();
        worker.join().unwrap();
        assert!(fired.try_recv().is_err(), "a stopped burst never fires");
    }

    #[test]
    fn the_trigger_names_its_session_and_stop_joins_the_thread() {
        let (signal, inbox) = mpsc::channel();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (worker, done) = spawn_debouncer(
            SessionId(4),
            true,
            WATCH_DEBOUNCE,
            inbox,
            || {},
            move |m| {
                let _ = tx.send(m);
            },
        )
        .unwrap();
        signal.send(change("/p/app/src/main.rs")).unwrap();
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
                session: SessionId(4),
                paths: vec![PathBuf::from("/p/app/src/main.rs")],
                hot: true,
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
        let roots = [root.clone()];
        let is_relevant_path = |path: &Path| is_relevant_path(&roots, path);
        assert!(is_relevant_path(&root.join("src/main.rs")));
        assert!(is_relevant_path(&root.join("src/ui/view.rs")));
        assert!(is_relevant_path(&root.join("Cargo.toml")));
        assert!(!is_relevant_path(&root.join("target/debug/app")));
        assert!(!is_relevant_path(&root.join("build/out.o")));
        assert!(!is_relevant_path(&root.join("src/.main.rs.swp")));
        assert!(!is_relevant_path(&root.join(".git/index")));
        assert!(!is_relevant_path(&root.join("src/main.rs~")));
        // A nested dir merely *named* `target` or `build` inside src/ is
        // still source: the rule is judged against the package directory
        // (`start` seeds the roots with it; `resolve_hot_scope` adds the
        // workspace root and every package directory), never a src/ tree.
        assert!(is_relevant_path(&root.join("src/target/mod.rs")));
        assert!(is_relevant_path(&root.join("src/build/mod.rs")));
    }

    #[test]
    fn reads_never_count_but_writes_do() {
        let root = PathBuf::from("/p/app");
        let roots = [root.clone()];
        let is_relevant_event = |event: &notify::Event| relevant_paths(&roots, event).is_some();
        let src = root.join("src/main.rs");
        let event = |kind| notify::Event::new(kind).add_path(src.clone());
        assert!(!is_relevant_event(&event(EventKind::Access(
            AccessKind::Open(AccessMode::Any)
        ))));
        assert!(!is_relevant_event(&event(EventKind::Access(
            AccessKind::Close(AccessMode::Read)
        ))));
        assert!(!is_relevant_event(&event(EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::AccessTime)
        ))));
        assert!(is_relevant_event(&event(EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        ))));
        assert!(is_relevant_event(&event(EventKind::Modify(
            ModifyKind::Data(DataChange::Any)
        ))));
        assert!(is_relevant_event(&event(EventKind::Create(
            CreateKind::File
        ))));
        // A write to a swap file is still noise.
        let swap = notify::Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(root.join("src/.main.rs.swp"));
        assert!(!is_relevant_event(&swap));
    }

    /// One change tick naming `path`.
    fn change(path: &str) -> Signal {
        Signal::Change(vec![PathBuf::from(path)])
    }

    #[test]
    fn a_burst_fires_once_with_every_path_it_touched_deduplicated() {
        let (signal, inbox) = mpsc::channel();
        let (fired_tx, fired) = mpsc::channel();
        let worker = thread::spawn(move || {
            debounce_loop(&inbox, WATCH_DEBOUNCE, |paths| {
                let _ = fired_tx.send(paths);
            });
        });
        signal.send(change("/p/app/src/b.rs")).unwrap();
        signal.send(change("/p/app/src/a.rs")).unwrap();
        signal.send(change("/p/app/src/b.rs")).unwrap();
        signal.send(Signal::Change(Vec::new())).unwrap();
        let paths = fired
            .recv_timeout(WATCH_DEBOUNCE * 4)
            .expect("the settled burst fires");
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/p/app/src/a.rs"),
                PathBuf::from("/p/app/src/b.rs")
            ]
        );

        // The next burst starts empty: nothing carries over.
        signal.send(change("/p/app/src/c.rs")).unwrap();
        let paths = fired
            .recv_timeout(WATCH_DEBOUNCE * 4)
            .expect("second burst fires");
        assert_eq!(paths, vec![PathBuf::from("/p/app/src/c.rs")]);
        signal.send(Signal::Stop).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn a_cargo_config_counts_and_other_paths_judge_by_their_deepest_root() {
        let workspace = PathBuf::from("/w");
        let member = PathBuf::from("/w/crates/ui");
        let outside = PathBuf::from("/deps/.hidden-parent/widgets");
        let roots = [workspace.clone(), member.clone(), outside.clone()];

        assert!(is_relevant_path(
            &roots,
            &workspace.join(".cargo/config.toml")
        ));
        assert!(is_relevant_path(&roots, &workspace.join(".cargo/config")));
        assert!(!is_relevant_path(
            &roots,
            &workspace.join(".cargo/credentials.toml")
        ));
        assert!(is_relevant_path(&roots, &member.join("src/lib.rs")));
        assert!(!is_relevant_path(&roots, &workspace.join("target/debug/x")));
        // A path dependency under a hidden directory is still source: its
        // own root is what it is judged against.
        assert!(is_relevant_path(&roots, &outside.join("src/lib.rs")));
        assert!(!is_relevant_path(&roots, &outside.join("src/.lib.rs.swp")));
    }

    #[test]
    fn the_hot_scope_covers_the_three_path_classes_and_nothing_whole() {
        let set = WatchSet {
            replayable: vec![PathBuf::from("/w/src"), PathBuf::from("/w/crates/ui/src")],
            local_non_member: vec![PathBuf::from("/deps/widgets/src")],
            build_inputs: vec![
                PathBuf::from("/w/Cargo.lock"),
                PathBuf::from("/w/Cargo.toml"),
                PathBuf::from("/w/build.rs"),
            ],
            roots: vec![PathBuf::from("/w"), PathBuf::from("/w/crates/ui")],
        };
        assert_eq!(
            hot_scope(&set),
            vec![
                (PathBuf::from("/w/src"), RecursiveMode::Recursive),
                (PathBuf::from("/w/crates/ui/src"), RecursiveMode::Recursive),
                (PathBuf::from("/deps/widgets/src"), RecursiveMode::Recursive),
                (PathBuf::from("/w/Cargo.lock"), RecursiveMode::NonRecursive),
                (PathBuf::from("/w/Cargo.toml"), RecursiveMode::NonRecursive),
                (PathBuf::from("/w/build.rs"), RecursiveMode::NonRecursive),
            ],
            "the src/ trees recursively (members, then non-members), \
             each build input as one file, no root or package dir whole"
        );
    }

    /// A single-package app at `/w` (lib + bin + build script) depending on
    /// the local non-member path package `widgets` at `/deps/widgets`.
    const METADATA: &str = r#"{
        "packages": [
            {"id": "path+file:///w#0.1.0", "name": "app", "source": null,
             "manifest_path": "/w/Cargo.toml",
             "targets": [
                {"name": "app", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/w/src/lib.rs"},
                {"name": "app", "kind": ["bin"], "crate_types": ["bin"], "src_path": "/w/src/main.rs"},
                {"name": "build-script-build", "kind": ["custom-build"], "crate_types": ["bin"], "src_path": "/w/build.rs"}]},
            {"id": "path+file:///deps/widgets#0.1.0", "name": "widgets", "source": null,
             "manifest_path": "/deps/widgets/Cargo.toml",
             "targets": [
                {"name": "widgets", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/deps/widgets/src/lib.rs"}]}
        ],
        "workspace_members": ["path+file:///w#0.1.0"],
        "resolve": {"nodes": [
            {"id": "path+file:///w#0.1.0", "deps": [
                {"name": "widgets", "pkg": "path+file:///deps/widgets#0.1.0",
                 "dep_kinds": [{"kind": null, "target": null}]}]},
            {"id": "path+file:///deps/widgets#0.1.0", "deps": []}],
         "root": "path+file:///w#0.1.0"},
        "workspace_root": "/w"
    }"#;

    #[test]
    fn the_hot_scope_watches_the_lockfile_frust_toml_build_script_and_non_member_src_but_not_target()
     {
        let graph = WorkspaceGraph::from_metadata(METADATA, "app", None).unwrap();
        let set = frust_drive::hotpatch::watch::watch_set(&graph);
        let scope = hot_scope(&set);
        for (path, mode) in [
            ("/w/Cargo.lock", RecursiveMode::NonRecursive),
            ("/w/frust.toml", RecursiveMode::NonRecursive),
            ("/w/build.rs", RecursiveMode::NonRecursive),
            ("/w/.cargo/config.toml", RecursiveMode::NonRecursive),
            ("/w/src", RecursiveMode::Recursive),
            ("/deps/widgets/src", RecursiveMode::Recursive),
        ] {
            assert!(
                scope.contains(&(PathBuf::from(path), mode)),
                "{path} ({mode:?}) in {scope:?}"
            );
        }
        for (path, _) in &scope {
            assert!(
                !path.starts_with("/w/target") && !path.starts_with("/deps/widgets/target"),
                "{path:?} is build output"
            );
            assert!(
                path != Path::new("/w") && path != Path::new("/deps/widgets"),
                "{path:?} is a package directory watched whole"
            );
        }
        // A burst inside a watched tree's build output, reported against the
        // widened roots, is still noise.
        assert!(!is_relevant_path(
            &set.roots,
            Path::new("/w/target/debug/app")
        ));
        assert!(is_relevant_path(&set.roots, Path::new("/w/Cargo.lock")));
        assert!(is_relevant_path(
            &set.roots,
            Path::new("/deps/widgets/src/lib.rs")
        ));
    }

    #[test]
    fn the_cold_scope_stays_the_projects_src_tree_and_manifest() {
        assert_eq!(
            base_scope(Path::new("/w")),
            vec![
                (PathBuf::from("/w/src"), RecursiveMode::Recursive),
                (PathBuf::from("/w/Cargo.toml"), RecursiveMode::NonRecursive),
            ]
        );
    }
}
