//! The shell side of the in-app devtools service: a [`DevtoolsBackend`]
//! implementation over each shell's own UI thread, plus the process-wide
//! start/pump/publish seam the three shells drive.
//!
//! Compiled **only** under this crate's `devtools` cargo feature. Without it
//! nothing here exists — no listener, no protocol symbols, no discovery-line
//! bytes (see the feature's comment in `Cargo.toml`). The gate is the feature,
//! never a `debug_assertions` runtime check.
//!
//! # Shape
//!
//! ```text
//!  service thread ─► backend thread ─► [request queue] ─► UI thread (pump)
//!  (frust-devtools)   (ShellBackend)    ▲   reply channel   ▲
//!                                       └──── one hop ──────┘
//! ```
//!
//! `frust-devtools` calls [`DevtoolsBackend`]'s plain sync methods on its own
//! backend thread. Everything that needs live app state — the widget tree,
//! one widget's props, an injected event — is answered by *hopping to the UI
//! thread*: the backend pushes a [`UiRequest`] onto a process-wide queue,
//! nudges the shell awake if that shell needs nudging, and blocks on a reply
//! channel. Each shell calls [`pump`] once per frame (desktop: on the woken
//! event-loop turn) with a [`DevtoolsUi`] view of itself, which drains the
//! queue, answers each request, and — for an injection — dispatches the
//! synthetic event through **the same `event` path real input uses**, so
//! nothing bypasses hit-testing, capture, focus routing, or the reactive
//! root-owner wrap.
//!
//! # Timeouts
//!
//! The service already caps every backend call at
//! `ServiceConfig::backend_timeout` (1s) and answers the waiting client with an
//! error when it expires, so a frozen UI thread costs a client one error and
//! nothing more. This module deliberately adds **no competing client-facing
//! timeout**: [`UI_HOP_DEADLINE`] is five times that budget and exists only as
//! a liveness backstop so a permanently dead UI thread cannot wedge the single
//! backend thread for the rest of the process — the client has long since been
//! answered by the service's own timeout in that case.
//!
//! # Metrics
//!
//! [`ShellBackend::metrics_snapshot`] needs no hop: uptime comes from an
//! [`Instant`] captured at [`start`], and RSS is read from `/proc/self/status`
//! (Linux/Android; `None` everywhere else — the protocol models it optional
//! precisely because it is platform-dependent).
//!
//! # Screenshot
//!
//! Not supported in v1 — the trait's default (`NOT_SUPPORTED` on the wire),
//! and the default `handshake_info` capability set that pairs with it, are both
//! left untouched.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use frust_core::InspectNode;
use frust_core::event::{
    InputEvent, Key, KeyEvent, Modifiers, PointerButton, PointerEvent, PointerPhase, ScrollDelta,
};
use frust_devtools::{AppInfo, BackendError, DevtoolsBackend, Service, ServiceHandle};
use frust_devtools_protocol::{
    FrameStats, InputScrollParams, InputTapParams, MetricsSnapshot, RectPx, WidgetNode,
    WidgetProps, WidgetTreeDump,
};
use kurbo::Point;

use crate::perf::FramePasses;

/// Runtime kill switch: set `FRUST_DEVTOOLS=0` to skip starting the service in
/// a build that compiled it in. Parsed exactly like `FRUST_TRACE`/the
/// `FRUST_NO_*` frame-pipeline switches (see `crate::perf`): only the literal
/// string `"0"` disables — anything else, including unset, leaves devtools on.
pub const DEVTOOLS_VAR: &str = "FRUST_DEVTOOLS";

/// Liveness backstop for one UI-thread hop — **not** a client-facing timeout
/// (see the module docs). Deliberately far longer than the service's own 1s
/// `backend_timeout`, so the client always learns "the UI thread is busy" from
/// the service, and this only ever fires to release a backend thread parked
/// behind a UI thread that is never coming back.
const UI_HOP_DEADLINE: Duration = Duration::from_secs(5);

/// Cap on how many requests may queue for the UI thread. A client that floods
/// the service is already bounded by `ServiceConfig::backend_queue_depth` (one
/// call runs at a time), so this only guards against a UI thread that stopped
/// pumping entirely (backgrounded app) while calls keep timing out and
/// abandoning their queue slots.
const MAX_QUEUED_REQUESTS: usize = 64;

// ---------------------------------------------------------------------
// The UI-thread hop
// ---------------------------------------------------------------------

/// One question the backend thread needs the UI thread to answer. Each variant
/// carries its own reply channel; the sender is dropped without a value if the
/// pump cannot answer, which the waiting side reads as "no answer".
enum UiRequest {
    /// Snapshot the retained tree.
    Tree(std::sync::mpsc::Sender<Vec<InspectNode>>),
    /// Deliver synthetic input events, in order, through the real event path.
    Events(Vec<InputEvent>, std::sync::mpsc::Sender<()>),
}

/// The process-wide hop queue plus each shell's optional wake mechanism.
struct Bridge {
    queue: Mutex<VecDeque<UiRequest>>,
    /// How to nudge the UI thread when a request is queued. `Some` for a shell
    /// whose loop idles until something wakes it (desktop's winit
    /// `ControlFlow::Wait`, woken through its `EventLoopProxy`); `None` for the
    /// two mobile shells, whose Choreographer/`CADisplayLink` loop ticks
    /// continuously while resumed and drains the queue at the top of every
    /// frame — a hop there costs at most one frame interval.
    wake: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Bridge {
    /// Queue `request` and wake the UI thread. Returns `false` when the queue
    /// is at [`MAX_QUEUED_REQUESTS`] (the request is dropped, the caller
    /// reports the failure).
    fn submit(&self, request: UiRequest) -> bool {
        {
            let Ok(mut queue) = self.queue.lock() else {
                return false;
            };
            if queue.len() >= MAX_QUEUED_REQUESTS {
                return false;
            }
            queue.push_back(request);
        }
        if let Some(wake) = self.wake.as_ref() {
            wake();
        }
        true
    }
}

/// Installed once by [`start`]; read by both the backend thread ([`Bridge::submit`])
/// and the UI thread ([`pump`]).
static BRIDGE: OnceLock<Arc<Bridge>> = OnceLock::new();

/// The running service. Held for the process lifetime deliberately: devtools
/// should be available for as long as the app is, and there is no shutdown
/// point a shell could hand it (`frust_destroy`/window close are also
/// process-teardown on every shipped shell).
static SERVICE: OnceLock<ServiceHandle> = OnceLock::new();

/// Monotonic frame counter for the published [`FrameStats::n`] — independent of
/// `perf::FrameStats`'s own counter, which only advances when perf tracing is
/// enabled (devtools frame stats must flow regardless of `FRUST_TRACE`).
static FRAME_N: AtomicU64 = AtomicU64::new(0);

/// What a shell exposes to the devtools pump: read the retained tree, and
/// deliver one synthetic event through the shell's ordinary input path.
///
/// Deliberately tiny — everything else a backend answers (metrics, handshake)
/// needs no app state at all. Implemented by each shell over whatever it holds:
/// the desktop `ShellHandler` over its own `RenderRoot`, the mobile handles
/// over their `Box<dyn AppTree>`.
pub trait DevtoolsUi {
    /// A read-only snapshot of the retained widget tree
    /// (`RenderRoot::inspect`).
    fn inspect(&self) -> Vec<InspectNode>;

    /// Deliver one synthetic input event.
    ///
    /// **Contract:** this must route through exactly the same path a real
    /// platform event takes — the shell's own `event`/`dispatch` helper, under
    /// the reactive root owner, setting whatever frame-gate latch real input
    /// sets. An implementation that reaches into widget code directly would
    /// bypass hit-testing, capture and focus routing, which is the whole reason
    /// injection is expressed as an event rather than a call.
    fn dispatch(&mut self, event: InputEvent);
}

/// Drain every queued devtools request, answering each against `ui`.
///
/// Called by each shell once per frame (or, on desktop, on the event-loop turn
/// the wake produced). A no-op — one uncontended `Mutex` check — when nothing
/// is queued, which is every frame of a session with no devtools client
/// attached.
///
/// **Never blocks or parks the UI thread** (`docs/REVIEW_FOCUS.md` rates that
/// critical): it only ever *sends* on a reply channel, never receives, and the
/// one lock it takes is held for a `pop_front` — released before any app code
/// runs. All the waiting in this design happens on the backend thread, which
/// is exactly what that thread exists for.
pub fn pump(ui: &mut dyn DevtoolsUi) {
    let Some(bridge) = BRIDGE.get() else {
        return;
    };
    loop {
        let request = {
            let Ok(mut queue) = bridge.queue.lock() else {
                return;
            };
            // The lock is released before answering: `dispatch` re-enters app
            // code, which must never run while holding the queue lock.
            match queue.pop_front() {
                Some(request) => request,
                None => return,
            }
        };
        match request {
            UiRequest::Tree(reply) => {
                // A dropped receiver (the client timed out) is expected, not an
                // error — see `frust_devtools::hop`'s blocking model.
                let _ = reply.send(ui.inspect());
            }
            UiRequest::Events(events, reply) => {
                for event in events {
                    ui.dispatch(event);
                }
                let _ = reply.send(());
            }
        }
    }
}

// ---------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------

/// Start the devtools service for this process, once.
///
/// `app_name` identifies the app at handshake; `wake` is the shell's
/// UI-thread nudge (see [`Bridge::wake`] — `None` for a continuously ticking
/// mobile loop). Call this **after** the shell's logger is installed: the
/// service logs its discovery line (`frust-devtools listening on <port> token
/// <token>`) through the `log` facade, and tooling recovers both the port and
/// the handshake token by grepping that line out of stderr/logcat. The token
/// is the service's own secret — this module only passes the log line along
/// (and offers [`token`] to the in-process caller that already owns it).
///
/// Never load-bearing: a bind failure, a repeat call, or the `FRUST_DEVTOOLS=0`
/// kill switch all leave the app running exactly as it would without devtools.
pub fn start(app_name: impl Into<String>, wake: Option<Box<dyn Fn() + Send + Sync>>) {
    if kill_switch_engaged(std::env::var(DEVTOOLS_VAR).ok().as_deref()) {
        log::info!("frust-devtools: disabled by {DEVTOOLS_VAR}=0");
        return;
    }
    if SERVICE.get().is_some() {
        return;
    }

    let bridge = Arc::new(Bridge {
        queue: Mutex::new(VecDeque::new()),
        wake,
    });
    // A second `start` in one process (an Android activity recreated against a
    // live process) must not install a second bridge behind the first service.
    if BRIDGE.set(Arc::clone(&bridge)).is_err() {
        return;
    }

    let backend = ShellBackend {
        bridge,
        started: Instant::now(),
    };
    let info = AppInfo::new(app_name, env!("CARGO_PKG_VERSION"));
    match Service::start(backend, info) {
        Ok(handle) => {
            let _ = SERVICE.set(handle);
        }
        Err(err) => log::warn!(
            "{}",
            frust_devtools_protocol::format_failure_line(&err.to_string())
        ),
    }
}

/// The loopback port the service bound, or `None` when it never started
/// (kill switch, bind failure, or a shell that does not start it).
///
/// The in-process equivalent of grepping the discovery line out of a log
/// stream — what a test, or an app that wants to surface the port in its own
/// UI, reads.
pub fn port() -> Option<u16> {
    SERVICE.get().map(ServiceHandle::port)
}

/// The handshake token this process's service requires, or `None` when no
/// service is running (see [`port`]) or it was started with auth off.
///
/// The token is otherwise **service-internal**: this module neither generates
/// nor checks it, and nothing in the shell needs it — the accessor exists for
/// the in-process caller that is already inside the trust boundary (a test
/// driving a loopback client, or an app choosing to surface it next to the
/// port). Never send it anywhere the discovery line does not already go.
pub fn token() -> Option<String> {
    SERVICE
        .get()
        .and_then(ServiceHandle::token)
        .map(str::to_string)
}

/// The kill-switch decision, pure so it is testable without touching the
/// process environment: only the literal `"0"` disables (mirroring
/// `perf::trace_switch`'s "not the string zero" convention, inverted — this
/// switch is default-ON once compiled in).
fn kill_switch_engaged(value: Option<&str>) -> bool {
    value == Some("0")
}

/// A best-effort app name for the handshake, derived from the running process:
/// `/proc/self/cmdline`'s first entry on Linux/Android (on Android that is the
/// package name), else `argv[0]`, reduced to its file name. Falls back to
/// `"frust-app"` when neither is readable.
///
/// A shell that knows a better name should pass its own — nothing here is
/// authoritative, it is a label a developer reads in a client.
pub fn app_name_from_process() -> String {
    let raw = std::fs::read_to_string("/proc/self/cmdline")
        .ok()
        .and_then(|cmdline| {
            cmdline
                .split('\0')
                .next()
                .filter(|first| !first.is_empty())
                .map(str::to_string)
        })
        .or_else(|| std::env::args().next());

    raw.as_deref()
        .and_then(|path| path.rsplit(['/', '\\']).next())
        .filter(|name| !name.is_empty())
        .unwrap_or("frust-app")
        .to_string()
}

// ---------------------------------------------------------------------
// Frame stats
// ---------------------------------------------------------------------

/// Publish one frame's timings to every subscribed devtools client.
///
/// Called from `perf::FrameStats::record` — the single site every shell's
/// frame pipeline already funnels through (desktop/Android/iOS, inline and
/// render-thread-split alike), so no shell has its own copy to keep in sync.
/// It sits **before** that method's `perf::enabled()` early return on purpose:
/// devtools frame stats must flow whether or not `FRUST_TRACE` is set, since a
/// client subscribing to them is its own opt-in.
///
/// Non-blocking by construction (`ServiceHandle::publish_frame_stats` writes
/// into a bounded drop-oldest ring), and with no service running it is one
/// relaxed atomic load.
pub(crate) fn publish_frame(passes: &FramePasses) {
    let Some(service) = SERVICE.get() else {
        return;
    };
    let n = FRAME_N.fetch_add(1, Ordering::Relaxed) + 1;
    service.publish_frame_stats(FrameStats {
        n,
        total_us: passes.total().as_micros() as u64,
        rebuild_us: passes.rebuild.as_micros() as u64,
        layout_us: passes.layout.as_micros() as u64,
        paint_us: passes.paint.as_micros() as u64,
        encode_us: passes.encode.as_micros() as u64,
        acquire_us: passes.acquire.as_micros() as u64,
        submit_us: passes.submit.as_micros() as u64,
        skipped: passes.skipped,
    });
}

// ---------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------

/// The [`DevtoolsBackend`] every shell shares: a hop to the UI thread for
/// anything that touches app state, answered locally for anything that does
/// not.
struct ShellBackend {
    bridge: Arc<Bridge>,
    /// The epoch `metrics_snapshot`'s uptime is measured from — process start
    /// as far as devtools is concerned (the shell starts the service during
    /// its own init).
    started: Instant,
}

impl ShellBackend {
    /// Run one UI-thread hop: queue `make_request`'s value, block on the reply
    /// channel up to [`UI_HOP_DEADLINE`], and report what came back.
    fn hop<T>(
        &self,
        make_request: impl FnOnce(std::sync::mpsc::Sender<T>) -> UiRequest,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        if !self.bridge.submit(make_request(tx)) {
            log::warn!("frust-devtools: the UI-thread request queue is full");
            return None;
        }
        rx.recv_timeout(UI_HOP_DEADLINE).ok()
    }
}

impl DevtoolsBackend for ShellBackend {
    fn widget_tree(&self) -> WidgetTreeDump {
        // Infallible by contract: an unanswered hop reports an empty tree, the
        // same thing a not-yet-built app reports.
        match self.hop(UiRequest::Tree) {
            Some(nodes) => tree_dump(&nodes),
            None => WidgetTreeDump { roots: Vec::new() },
        }
    }

    fn widget_props(&self, id: u64) -> Option<WidgetProps> {
        let nodes = self.hop(UiRequest::Tree)?;
        props_for(&nodes, id)
    }

    fn metrics_snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            rss_bytes: rss_bytes(),
            uptime_ms: self.started.elapsed().as_millis() as u64,
        }
    }

    fn inject_tap(&self, params: InputTapParams) -> Result<(), BackendError> {
        let position = logical_point(params.x, params.y)?;
        // A tap is a full gesture, not a lone `Down`: every interactive widget
        // in `frust-widgets` fires on **up-inside** (docs/CODE_STANDARDS.md's
        // Interaction Semantics), so a `Down` alone would only paint a pressed
        // state and never activate anything.
        self.inject(vec![
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Down,
                position,
                button: PointerButton::Primary,
            }),
            InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Up,
                position,
                button: PointerButton::Primary,
            }),
        ])
    }

    fn inject_scroll(&self, params: InputScrollParams) -> Result<(), BackendError> {
        let position = logical_point(params.x, params.y)?;
        if !params.dx.is_finite() || !params.dy.is_finite() {
            return Err(BackendError::invalid_request(
                "scroll delta must be finite logical px",
            ));
        }
        // Pixels, not Lines: the wire delta is documented as logical px, which
        // is what a trackpad/touch scroll already reports.
        self.inject(vec![InputEvent::Scroll {
            position,
            delta: ScrollDelta::Pixels(params.dx, params.dy),
        }])
    }

    fn inject_text(&self, text: &str) -> Result<(), BackendError> {
        if text.is_empty() {
            return Err(BackendError::invalid_request("text must not be empty"));
        }
        // `Key::Character` carries the already-resolved text a key produced —
        // the same shape the desktop shell builds from winit's `KeyEvent.text`
        // — and is focus-routed, so an app with nothing focused simply ignores
        // it (not an error the client has to distinguish).
        self.inject(vec![InputEvent::Key(KeyEvent {
            key: Key::Character(text.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })])
    }
}

impl ShellBackend {
    /// Hop a batch of synthetic events onto the UI thread and wait for them to
    /// have been dispatched, so the ack a client receives means "delivered",
    /// not "queued".
    fn inject(&self, events: Vec<InputEvent>) -> Result<(), BackendError> {
        match self.hop(|reply| UiRequest::Events(events, reply)) {
            Some(()) => Ok(()),
            None => Err(BackendError::unavailable(
                "the app's UI thread did not process the injected event",
            )),
        }
    }
}

/// Validate an injected logical-px coordinate. A non-finite coordinate would
/// poison hit-testing rather than miss, so it is rejected as a bad request
/// instead of being clamped.
fn logical_point(x: f64, y: f64) -> Result<Point, BackendError> {
    if !x.is_finite() || !y.is_finite() {
        return Err(BackendError::invalid_request(
            "tap/scroll position must be finite logical px",
        ));
    }
    Ok(Point::new(x, y))
}

/// Best-effort resident-set size in bytes, read from `/proc/self/status`'s
/// `VmRSS` (reported in kB). `None` on any platform without procfs, and on any
/// read/parse failure — the protocol models this optional for exactly that
/// reason.
fn rss_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        parse_vm_rss_kb(&status).map(|kb| kb * 1024)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}

/// Pull `VmRSS`'s kB value out of a `/proc/self/status` body. Separated from
/// the read so the parse is unit-testable against a fixture on any host.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "android", test)),
    allow(dead_code)
)]
fn parse_vm_rss_kb(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse::<u64>().ok())
}

// ---------------------------------------------------------------------
// InspectNode -> protocol mapping
// ---------------------------------------------------------------------

/// Fold `frust-core`'s flat, pre-order [`InspectNode`] snapshot into the
/// protocol's nested [`WidgetTreeDump`].
///
/// The two shapes differ on purpose: core reports a flat list with parent/child
/// **ids** (nothing serialization-shaped in `frust-core`), while the wire type
/// nests so a client renders a tree with no reconstruction pass. Roots are the
/// nodes core reported with no parent, in snapshot order.
///
/// A `visited` set guards the descent: the snapshot is a tree by construction,
/// but this walk is driven by ids and must terminate even against a malformed
/// one rather than recursing until the stack dies.
fn tree_dump(nodes: &[InspectNode]) -> WidgetTreeDump {
    let index: HashMap<u64, usize> = nodes
        .iter()
        .enumerate()
        .map(|(slot, node)| (node.id.0, slot))
        .collect();
    let mut visited = HashSet::new();
    let roots = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent.is_none())
        .filter_map(|(slot, _)| widget_node(nodes, &index, slot, &mut visited))
        .collect();
    WidgetTreeDump { roots }
}

fn widget_node(
    nodes: &[InspectNode],
    index: &HashMap<u64, usize>,
    slot: usize,
    visited: &mut HashSet<u64>,
) -> Option<WidgetNode> {
    let node = nodes.get(slot)?;
    if !visited.insert(node.id.0) {
        return None;
    }
    let children = node
        .children
        .iter()
        .filter_map(|child| index.get(&child.0).copied())
        .filter_map(|child_slot| widget_node(nodes, index, child_slot, visited))
        .collect();
    Some(WidgetNode {
        id: node.id.0,
        type_name: node.type_name.to_string(),
        debug_label: node.debug_label.as_ref().map(|label| label.to_string()),
        // Always reported, never `None`: core's bounds are `Rect::ZERO`-sized
        // until a layout pass has run, and "zero-sized because nothing is laid
        // out yet" is more useful to a client than an absent field.
        bounds: Some(RectPx {
            x: node.bounds.x0,
            y: node.bounds.y0,
            width: node.bounds.width(),
            height: node.bounds.height(),
        }),
        children,
    })
}

/// One node's inspectable properties, or `None` when `id` names no node in this
/// snapshot (a stale id from a tree the app has since rebuilt — the service
/// turns that into `INVALID_PARAMS`).
///
/// The entry set is deliberately flat `(name, rendered value)` pairs: the
/// protocol carries strings, and a widget's real props are behind a `dyn Widget`
/// with no introspection seam, so what is reportable today is exactly what the
/// tree snapshot itself carries.
fn props_for(nodes: &[InspectNode], id: u64) -> Option<WidgetProps> {
    let node = nodes.iter().find(|node| node.id.0 == id)?;
    let mut entries = vec![("type".to_string(), node.type_name.to_string())];
    if let Some(label) = node.debug_label.as_ref() {
        entries.push(("debug_label".to_string(), label.to_string()));
    }
    entries.push(("x".to_string(), format!("{:.1}", node.bounds.x0)));
    entries.push(("y".to_string(), format!("{:.1}", node.bounds.y0)));
    entries.push(("width".to_string(), format!("{:.1}", node.bounds.width())));
    entries.push(("height".to_string(), format!("{:.1}", node.bounds.height())));
    entries.push(("depth".to_string(), node.depth.to_string()));
    entries.push((
        "parent".to_string(),
        node.parent
            .map(|parent| parent.0.to_string())
            .unwrap_or_else(|| "none".to_string()),
    ));
    entries.push(("children".to_string(), node.children.len().to_string()));
    Some(WidgetProps { id, entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_core::view::WidgetId;
    use kurbo::Rect;

    fn node(id: u64, parent: Option<u64>, children: &[u64], depth: usize) -> InspectNode {
        InspectNode {
            id: WidgetId(id),
            parent: parent.map(WidgetId),
            type_name: "StubWidget",
            debug_label: None,
            bounds: Rect::new(0.0, 0.0, 10.0, 20.0),
            children: children.iter().copied().map(WidgetId).collect(),
            depth,
        }
    }

    /// The fixture shape a real `RenderRoot::inspect()` produces: pre-order,
    /// one root, a child reached through `visit_children`, and a grandchild
    /// below it.
    fn fixture() -> Vec<InspectNode> {
        vec![
            InspectNode {
                bounds: Rect::new(0.0, 0.0, 100.0, 200.0),
                debug_label: Some("root".into()),
                ..node(1, None, &[2], 0)
            },
            node(2, Some(1), &[3], 1),
            node(3, Some(2), &[], 2),
        ]
    }

    #[test]
    fn tree_dump_nests_the_flat_pre_order_snapshot() {
        let dump = tree_dump(&fixture());
        assert_eq!(dump.roots.len(), 1);
        let root = &dump.roots[0];
        assert_eq!(root.id, 1);
        assert_eq!(root.type_name, "StubWidget");
        assert_eq!(root.debug_label.as_deref(), Some("root"));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].id, 2);
        assert_eq!(root.children[0].children[0].id, 3);
        assert!(root.children[0].children[0].children.is_empty());
    }

    #[test]
    fn tree_dump_carries_absolute_bounds_in_logical_px() {
        let dump = tree_dump(&fixture());
        let bounds = dump.roots[0].bounds.expect("bounds always reported");
        assert_eq!(bounds.x, 0.0);
        assert_eq!(bounds.y, 0.0);
        assert_eq!(bounds.width, 100.0);
        assert_eq!(bounds.height, 200.0);
    }

    #[test]
    fn tree_dump_of_an_empty_snapshot_is_an_empty_dump() {
        assert!(tree_dump(&[]).roots.is_empty());
    }

    #[test]
    fn tree_dump_reports_every_root_of_a_multi_root_snapshot() {
        let nodes = vec![node(1, None, &[], 0), node(2, None, &[], 0)];
        let dump = tree_dump(&nodes);
        assert_eq!(
            dump.roots.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn tree_dump_terminates_on_a_malformed_cyclic_snapshot() {
        // Not producible by `RenderRoot::inspect`, but the walk is id-driven:
        // a cycle must end the descent, not the process.
        let nodes = vec![node(1, None, &[2], 0), node(2, Some(1), &[1], 1)];
        let dump = tree_dump(&nodes);
        assert_eq!(dump.roots.len(), 1);
        assert!(dump.roots[0].children[0].children.is_empty());
    }

    #[test]
    fn tree_dump_skips_a_child_id_with_no_node() {
        let nodes = vec![node(1, None, &[2, 9], 0), node(2, Some(1), &[], 1)];
        let dump = tree_dump(&nodes);
        assert_eq!(dump.roots[0].children.len(), 1);
    }

    #[test]
    fn props_for_reports_geometry_and_lineage() {
        let props = props_for(&fixture(), 2).expect("node 2 is in the snapshot");
        assert_eq!(props.id, 2);
        let entries: HashMap<&str, &str> = props
            .entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(entries["type"], "StubWidget");
        assert_eq!(entries["width"], "10.0");
        assert_eq!(entries["height"], "20.0");
        assert_eq!(entries["depth"], "1");
        assert_eq!(entries["parent"], "1");
        assert_eq!(entries["children"], "1");
        assert!(!entries.contains_key("debug_label"));
    }

    #[test]
    fn props_for_reports_a_root_with_no_parent() {
        let props = props_for(&fixture(), 1).expect("node 1 is in the snapshot");
        let parent = props
            .entries
            .iter()
            .find(|(k, _)| k == "parent")
            .map(|(_, v)| v.as_str());
        assert_eq!(parent, Some("none"));
    }

    #[test]
    fn props_for_unknown_id_is_none() {
        assert!(props_for(&fixture(), 999).is_none());
    }

    #[test]
    fn a_non_finite_injection_coordinate_is_rejected() {
        // Never clamped: a NaN/infinite coordinate poisons every hit-test
        // comparison it reaches, so it must fail as a bad request instead.
        assert!(logical_point(f64::NAN, 0.0).is_err());
        assert!(logical_point(0.0, f64::INFINITY).is_err());
        assert_eq!(logical_point(3.0, 4.0).map(|p| (p.x, p.y)), Ok((3.0, 4.0)));
    }

    #[test]
    fn kill_switch_engages_only_on_literal_zero() {
        assert!(kill_switch_engaged(Some("0")));
        assert!(!kill_switch_engaged(None));
        assert!(!kill_switch_engaged(Some("1")));
        assert!(!kill_switch_engaged(Some("")));
    }

    #[test]
    fn vm_rss_is_parsed_in_kilobytes() {
        let status = "Name:\tapp\nVmPeak:\t  100 kB\nVmRSS:\t   4096 kB\nThreads:\t3\n";
        assert_eq!(parse_vm_rss_kb(status), Some(4096));
    }

    #[test]
    fn vm_rss_absent_is_none() {
        assert_eq!(parse_vm_rss_kb("Name:\tapp\nThreads:\t3\n"), None);
    }

    /// A `pump` with no service started must be a no-op rather than a panic —
    /// the state every host unit test and every non-devtools frame is in.
    #[test]
    fn pump_without_a_started_service_is_inert() {
        struct NoUi;
        impl DevtoolsUi for NoUi {
            fn inspect(&self) -> Vec<InspectNode> {
                panic!("pump must not touch the UI with nothing queued");
            }
            fn dispatch(&mut self, _event: InputEvent) {
                panic!("pump must not touch the UI with nothing queued");
            }
        }
        pump(&mut NoUi);
    }
}
