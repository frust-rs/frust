//! Network section: the lifecycle and connectivity gate harness. Gates drive
//! the device (background/foreground, minimize, close, switch networks) and
//! read this page, or grep the log for `frust-gate network:`.
//!
//! The page shows (a) the live [`AppLifecycleState`] via a tracked
//! [`app_lifecycle`] read, (b) [`Connectivity::current`], refreshed on every
//! event, (c) a newest-first log (capped at [`LOG_CAP`] rows, timestamped in
//! ms since the page mounted) of every [`AppLifecycleListener`] callback and
//! every [`ConnectivityEvent`], and (d) a "Block exit" checkbox: while on,
//! `on_exit_requested` returns [`AppExitResponse::Cancel`].
//!
//! # Threading
//!
//! Connectivity callbacks run on the plugin's own events thread; lifecycle
//! callbacks run on the UI thread. Both only format a row, write it once via
//! `log::info!` (prefix `frust-gate network:`) and send it over an mpsc
//! channel. A UI-thread [`spawn_local`] loop awaits the channel through
//! [`spawn_blocking`] and is the only writer of the log signal, so `build`
//! stays read-only. The loop ends when the listeners (the senders) drop with
//! the page state.
//!
//! # Timers
//!
//! A Timers block arms [`Timer`]s and a scoped sleep; every firing emits one
//! row (with the elapsed time since arming) through the same sink, so timer
//! firings interleave with lifecycle rows. The flagged timers fire inside the
//! foreground-exit callback; the marker variant writes a file synchronously
//! there, and the page reports the marker found at launch.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use frust::paths::{atomic_write, data_dir};
use frust::time::{Duration, Timer, sleep};
use frust::{
    AppExitResponse, AppLifecycleListener, BackgroundTask, ButtonStyle, Component, EdgeInsets, Get,
    Padding, RwSignal, Set, SizedBox, Theme, Update, View, app_lifecycle, button, checkbox, column,
    component, spawn_blocking, spawn_local, spawn_local_scoped, text, use_context,
};
use frust_connectivity::{Connectivity, ConnectivityEvent, ListenerHandle, NetworkPath};
use frust_database::{Database, DatabaseError, Value};

/// Most rows the log keeps; the oldest fall off the end.
const LOG_CAP: usize = 50;

/// File name of the marker the flagged-write timer leaves in the data dir.
const MARKER_NAME: &str = "timer-marker.txt";

/// Creates the table the flush-write buttons log into.
const FLUSH_LOG_DDL: &str = "CREATE TABLE IF NOT EXISTS flush_log \
     (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, at INTEGER NOT NULL)";

/// The `flush_log.kind` values, in the order their evidence rows are emitted.
const FLUSH_KINDS: [&str; 2] = ["guarded", "plain"];

/// The grep prefix every row carries in the platform log.
const LOG_PREFIX: &str = "frust-gate network:";

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &crate::PlaygroundState) -> impl View<crate::PlaygroundState> {
    component(NetworkPage)
}

/// Timestamps and emits rows from any thread.
#[derive(Clone)]
struct RowSink {
    tx: Sender<String>,
    start: Instant,
}

impl RowSink {
    fn emit(&self, message: &str) {
        let ms = self.start.elapsed().as_millis();
        let row = format!("[{ms:>7} ms] {message}");
        log::info!("{LOG_PREFIX} {row}");
        let _ = self.tx.send(row);
    }
}

#[derive(Default)]
struct NetworkPage;

struct NetworkPageState {
    log: RwSignal<Vec<String>>,
    current: RwSignal<Option<NetworkPath>>,
    block_exit: Arc<AtomicBool>,
    block_exit_sig: RwSignal<bool>,
    sink: RowSink,
    timers: Vec<Timer>,
    armed_at: Option<Instant>,
    _lifecycle: AppLifecycleListener,
    _connectivity: ListenerHandle,
}

impl Component for NetworkPage {
    type State = NetworkPageState;

    fn init(&self) -> NetworkPageState {
        let (tx, rx) = channel();
        let sink = RowSink {
            tx,
            start: Instant::now(),
        };
        let block_exit = Arc::new(AtomicBool::new(false));
        let log = RwSignal::new(Vec::new());

        sink.emit(&Self::marker_row());
        Self::report_flush_rows(&sink);
        let lifecycle = Self::lifecycle(&sink, &block_exit);
        // Registered before the first `Connectivity::current()` read, so the
        // monitor's `initial: true` report reaches this listener.
        let connectivity = Self::connectivity(&sink);
        let current = RwSignal::new(Connectivity::current());
        Self::drain(rx, log, current);

        NetworkPageState {
            log,
            current,
            block_exit,
            block_exit_sig: RwSignal::new(false),
            sink,
            timers: Vec::new(),
            armed_at: None,
            _lifecycle: lifecycle,
            _connectivity: connectivity,
        }
    }

    fn build(&self, state: &mut NetworkPageState) -> impl View<NetworkPageState> {
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let accent = theme.scheme().primary;
        let muted = theme.scheme().on_surface_variant;

        let lifecycle_line = format!("Lifecycle: {:?}", app_lifecycle().state());
        let network_line = Self::network_line(state.current.get());
        let blocked = state.block_exit_sig.get();

        let mut rows = column();
        for row in state.log.get() {
            rows = rows.child(text(row).size(11.0));
        }

        Padding(
            EdgeInsets::all(16.0),
            column()
                .child(text("Network").size(13.0).color(accent))
                .child(
                    text(
                        "Lifecycle and connectivity gate harness. Every callback and event \
                         is logged below, newest first, and in the platform log under \
                         \"frust-gate network:\".",
                    )
                    .size(11.0)
                    .color(muted),
                )
                .child(SizedBox(None, Some(12.0)))
                .child(text(lifecycle_line).size(13.0))
                .child(text(network_line).size(13.0))
                .child(SizedBox(None, Some(8.0)))
                .child(Self::block_exit_toggle(state, blocked))
                .child(Self::clear_button(state))
                .child(SizedBox(None, Some(8.0)))
                .child(text("Timers").size(13.0).color(accent))
                .child(Self::timer_buttons(state))
                .child(SizedBox(None, Some(8.0)))
                .child(rows),
        )
    }
}

impl NetworkPage {
    /// Registers every lifecycle callback; each logs its own name.
    fn lifecycle(sink: &RowSink, block_exit: &Arc<AtomicBool>) -> AppLifecycleListener {
        let named = |name: &'static str| {
            let sink = sink.clone();
            move || sink.emit(name)
        };
        let state_sink = sink.clone();
        let exit_sink = sink.clone();
        let block = Arc::clone(block_exit);
        AppLifecycleListener::new()
            .on_show(named("on_show"))
            .on_resume(named("on_resume"))
            .on_hide(named("on_hide"))
            .on_inactive(named("on_inactive"))
            .on_pause(named("on_pause"))
            .on_restart(named("on_restart"))
            .on_detach(named("on_detach"))
            .on_state_change(move |new_state| {
                state_sink.emit(&format!("on_state_change -> {new_state:?}"));
            })
            .on_exit_requested(move || {
                if block.load(Ordering::SeqCst) {
                    exit_sink.emit("on_exit_requested: exit request cancelled");
                    AppExitResponse::Cancel
                } else {
                    exit_sink.emit("on_exit_requested: Exit");
                    AppExitResponse::Exit
                }
            })
    }

    /// Registers the connectivity listener (runs on the plugin events thread).
    fn connectivity(sink: &RowSink) -> ListenerHandle {
        let sink = sink.clone();
        Connectivity::set_listener(Box::new(move |event| {
            sink.emit(&Self::describe(event));
        }))
    }

    fn describe(event: ConnectivityEvent) -> String {
        match event {
            ConnectivityEvent::Available { path, initial } => format!(
                "connectivity Available {{ transport: {:?}, network: {}, initial: {initial} }}",
                path.transport,
                path.id.as_u64()
            ),
            ConnectivityEvent::Lost => "connectivity Lost".to_string(),
            other => format!("connectivity {other:?}"),
        }
    }

    fn network_line(current: Option<NetworkPath>) -> String {
        match current {
            Some(path) => format!(
                "Connectivity: {:?} network {} (expensive: {}, constrained: {})",
                path.transport,
                path.id.as_u64(),
                path.expensive,
                path.constrained
            ),
            None => "Connectivity: none".to_string(),
        }
    }

    /// The UI-thread loop that moves rows from the channel into the signals.
    fn drain(
        rx: Receiver<String>,
        log: RwSignal<Vec<String>>,
        current: RwSignal<Option<NetworkPath>>,
    ) {
        let rx = Arc::new(Mutex::new(rx));
        spawn_local(async move {
            loop {
                let rx = Arc::clone(&rx);
                let received = spawn_blocking(move || rx.lock().ok()?.recv().ok()).await;
                let Ok(Some(row)) = received else { break };
                log.update(|rows| {
                    rows.insert(0, row);
                    rows.truncate(LOG_CAP);
                });
                current.set(Connectivity::current());
            }
        });
    }

    fn block_exit_toggle(state: &NetworkPageState, blocked: bool) -> impl View<NetworkPageState> {
        let flag = Arc::clone(&state.block_exit);
        let sig = state.block_exit_sig;
        checkbox(
            blocked,
            "Block exit",
            move |_state: &mut NetworkPageState, requested: bool| {
                flag.store(requested, Ordering::SeqCst);
                sig.set(requested);
            },
        )
    }

    fn marker_path() -> Option<PathBuf> {
        data_dir().map(|dir| dir.join(MARKER_NAME))
    }

    /// The launch row describing the marker a previous run may have left.
    fn marker_row() -> String {
        match Self::marker_path().and_then(|path| std::fs::read_to_string(path).ok()) {
            Some(stamp) => format!("marker present: {}", stamp.trim()),
            None => "marker absent".to_string(),
        }
    }

    /// Emit the newest `flush_log` row of each kind, read off the UI thread. These
    /// rows are the evidence a gate reads after a relaunch.
    fn report_flush_rows(sink: &RowSink) {
        let sink = sink.clone();
        drop(spawn_blocking(move || {
            let rows = Self::newest_flush_rows().unwrap_or_else(|e| {
                sink.emit(&format!("flush_log read failed: {e}"));
                Vec::new()
            });
            for (kind, at) in rows {
                match at {
                    Some(at) => sink.emit(&format!("last flush row ({kind}): {at}")),
                    None => sink.emit(&format!("last flush row ({kind}): none")),
                }
            }
        }));
    }

    /// Blocking: ensure the table exists and read the newest `at` per kind.
    fn newest_flush_rows() -> Result<Vec<(&'static str, Option<i64>)>, DatabaseError> {
        let db = Database::open("playground")?;
        db.execute(FLUSH_LOG_DDL, ())?;
        let mut out = Vec::new();
        for kind in FLUSH_KINDS {
            let rows = db.query(
                "SELECT at FROM flush_log WHERE kind = ?1 ORDER BY id DESC LIMIT 1",
                [kind],
            )?;
            let at = rows
                .first()
                .and_then(|row| row.get(0))
                .and_then(|value| match value {
                    Value::Integer(n) => Some(*n),
                    _ => None,
                });
            out.push((kind, at));
        }
        Ok(out)
    }

    /// Blocking: insert one `flush_log` row of `kind` stamped with epoch-ms.
    fn write_flush_row(kind: &str) -> Result<(), DatabaseError> {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let db = Database::open("playground")?;
        db.execute(FLUSH_LOG_DDL, ())?;
        db.transaction(|txn| {
            txn.execute(
                "INSERT INTO flush_log (kind, at) VALUES (?1, ?2)",
                [Value::Text(kind.to_string()), Value::Integer(at)],
            )?;
            Ok(())
        })
    }

    /// A button that arms a timer and stores its handle on the page state.
    fn arm_button(
        label: &'static str,
        make: impl Fn(&RowSink, Instant) -> Timer + 'static,
    ) -> impl View<NetworkPageState> {
        button(label, move |state: &mut NetworkPageState| {
            let armed = Instant::now();
            state.armed_at = Some(armed);
            state.timers.push(make(&state.sink, armed));
        })
        .small()
    }

    fn timer_buttons(_state: &NetworkPageState) -> impl View<NetworkPageState> {
        let fired = |sink: &RowSink, label: &'static str, armed: Instant| {
            let sink = sink.clone();
            move || sink.emit(&format!("{label} after {} ms", armed.elapsed().as_millis()))
        };
        let plain_2 = Self::arm_button("Arm 2 s", move |sink, armed| {
            Timer::new(
                Duration::from_secs(2),
                fired(sink, "timer 2 s fired", armed),
            )
        });
        let flagged_10 = Self::arm_button("Arm 10 s (flush on background)", move |sink, armed| {
            Timer::new(
                Duration::from_secs(10),
                fired(sink, "timer 10 s flagged fired", armed),
            )
            .flush_on_background()
        });
        let plain_10 = Self::arm_button("Arm 10 s (plain)", move |sink, armed| {
            Timer::new(
                Duration::from_secs(10),
                fired(sink, "timer 10 s plain fired", armed),
            )
        });
        let marker_10 = Self::arm_button("Arm 10 s (flush + marker write)", |sink, armed| {
            let sink = sink.clone();
            Timer::new(Duration::from_secs(10), move || {
                let ms = armed.elapsed().as_millis();
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                let written = match Self::marker_path() {
                    Some(path) => atomic_write(&path, stamp.to_string().as_bytes()).is_ok(),
                    None => false,
                };
                sink.emit(&format!(
                    "timer 10 s marker fired after {ms} ms (stamp {stamp}, written: {written})"
                ));
            })
            .flush_on_background()
        });
        let guarded_10 = Self::arm_button("Arm 10 s (flush + task + DB write)", |sink, _| {
            let sink = sink.clone();
            Timer::new(Duration::from_secs(10), move || {
                // Begun on the UI thread; the write below never needs the UI queue.
                let task = BackgroundTask::begin("playground-save");
                let sink = sink.clone();
                drop(spawn_blocking(move || {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let result = Self::write_flush_row("guarded");
                    let lost = task.expired();
                    drop(task);
                    match (result, lost) {
                        (Ok(()), false) => sink.emit("flush write done (guarded)"),
                        (Ok(()), true) => sink.emit("flush write done (guarded, task expired)"),
                        (Err(e), _) => sink.emit(&format!("flush write failed (guarded): {e}")),
                    }
                }));
            })
            .flush_on_background()
        });
        let unguarded_10 = Self::arm_button("Arm 10 s (flush + DB write, no guard)", |sink, _| {
            let sink = sink.clone();
            Timer::new(Duration::from_secs(10), move || {
                let sink = sink.clone();
                drop(spawn_blocking(move || {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    match Self::write_flush_row("plain") {
                        Ok(()) => sink.emit("flush write done (plain)"),
                        Err(e) => sink.emit(&format!("flush write failed (plain): {e}")),
                    }
                }));
            })
            .flush_on_background()
        });
        let sleeper = button(
            "Sleep 2 s (spawn_local_scoped)",
            |state: &mut NetworkPageState| {
                let armed = Instant::now();
                state.armed_at = Some(armed);
                let sink = state.sink.clone();
                spawn_local_scoped(async move {
                    sleep(Duration::from_secs(2)).await;
                    sink.emit(&format!(
                        "sleep 2 s done after {} ms",
                        armed.elapsed().as_millis()
                    ));
                });
            },
        )
        .small();
        let cancel = button("Cancel all", |state: &mut NetworkPageState| {
            state.timers.clear();
            state.armed_at = None;
            state.sink.emit("timers cancelled");
        })
        .style(ButtonStyle::Secondary)
        .small();
        column()
            .child(plain_2)
            .child(flagged_10)
            .child(plain_10)
            .child(marker_10)
            .child(guarded_10)
            .child(unguarded_10)
            .child(sleeper)
            .child(cancel)
    }

    fn clear_button(state: &NetworkPageState) -> impl View<NetworkPageState> {
        let log = state.log;
        button("Clear log", move |_state: &mut NetworkPageState| {
            log.set(Vec::new());
        })
        .style(ButtonStyle::Secondary)
        .small()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_network_reads_none() {
        assert_eq!(NetworkPage::network_line(None), "Connectivity: none");
    }

    #[test]
    fn lost_event_is_described() {
        assert_eq!(
            NetworkPage::describe(ConnectivityEvent::Lost),
            "connectivity Lost"
        );
    }

    #[test]
    fn rows_are_timestamped_and_sent() {
        let (tx, rx) = channel();
        let sink = RowSink {
            tx,
            start: Instant::now(),
        };
        sink.emit("on_resume");
        let row = rx.recv().expect("row sent");
        assert!(
            row.starts_with('[') && row.ends_with("ms] on_resume"),
            "{row}"
        );
    }
}
