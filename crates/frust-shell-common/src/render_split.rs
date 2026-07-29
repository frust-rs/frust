//! Render-thread-split plumbing shared by every shell.
//!
//! # What lives here
//!
//! The split moves `encode→acquire→blit→present` off the UI thread onto a
//! dedicated render thread: the UI thread keeps
//! `rebuild→layout→paint`, then hands the finished [`Scene`] across. This
//! module is the *vocabulary* for that handoff — the shells
//! own the threads and the `wgpu`/`vello` resources, this crate owns the
//! platform-free channel types and the pure lifecycle/kill-switch logic they
//! coordinate through.
//!
//! - [`render_channel`] — the single UI→render link: a **depth-1, latest-wins**
//!   scene-handoff slot (a newer [`SceneFrame`] replaces an un-taken one; the
//!   render thread always takes the freshest, dropping stale frames) fused with
//!   a FIFO lifecycle-command queue behind **one** [`std::sync::Condvar`], so
//!   the render thread has a single wait point ([`RenderReceiver::wait_next`]).
//!   Depth 1 is deliberate — Flutter's merged-mode precedent shows pipeline
//!   depth drops to 1 when threads merge; deeper queues add latency
//!   for no mobile win.
//! - [`scene_return_channel`] — the reverse, render→UI give-back link: a
//!   **non-blocking, depth-1** [`Mutex`]-only slot (no
//!   [`Condvar`] — the UI thread only ever polls it, never parks) the render
//!   thread pushes a drained scene back through once it is done reading it, so
//!   a shell's split `submit_frame` can `Scene::reset()` and reuse the buffer
//!   next frame instead of reallocating one via `Scene::new()` every frame —
//!   restoring frust-scene's documented reuse contract (`scene.rs`'s
//!   `Scene::reset` docs) in split mode.
//! - [`RenderCommand`] / [`RenderEvent`] / [`RenderPhase`] — the surface
//!   lifecycle vocabulary (created/changed/destroyed/pause/resume) as **owned
//!   commands**, modelled on `frust-render`'s `SurfacePhase` machine: a pure,
//!   host-testable [`next_render_phase`] transition table gates whether the
//!   render thread [`may render`](RenderPhase::can_render).
//! - [`Ack`] / [`AckWaiter`] — the cross-thread acknowledgment barrier that
//!   makes [`RenderCommand::Pause`] and [`RenderCommand::SurfaceDestroyed`]
//!   *synchronous*: the UI thread blocks until the render thread has honored
//!   the command. This is the correctness anchor for two platform hazards:
//!   Android can destroy the `ANativeWindow` while the render
//!   thread still holds the surface, and iOS can kill a process that submits
//!   Metal work after the app backgrounds. Both are barriers, not shared
//!   mutable flags.
//! - [`SceneFrame`] / [`FrameMeta`] / [`SurfaceSize`] — the per-frame payload
//!   crossing the handoff: the scene plus the frame clock, the surface
//!   dimensions, and (for the single-emitter perf recording)
//!   the UI thread's [`UiSpans`] half of the frame timing, which the render
//!   thread folds together with its own [`RenderSpans`] via
//!   [`FramePasses::from_split`](crate::perf::FramePasses::from_split).
//! - [`render_thread_enabled`] / [`NO_RENDER_THREAD_VAR`] — the single kill
//!   switch the shells consult, parsed exactly like [`crate::frame_gate`]'s
//!   `FRUST_NO_FRAME_GATE` (compile-time define *or* runtime env, any non-`"0"`
//!   value). When engaged, a shell keeps the pre-split single-thread path (kept
//!   as an escape hatch until the split's on-device throughput is fully
//!   validated).
//!
//! # Layering choice
//!
//! Like [`crate::perf`] and [`crate::frame_gate`], this is shell-owned and
//! platform-free: it takes **no** `frust-render`/`wgpu`/`vello` dependency, no
//! `unsafe`, and no `frust-reactive`, preserving this crate's
//! compiles-everywhere, reactive-free charter (see `docs/ARCHITECTURE.md`'s
//! Layer Dependencies). The scene payload ([`SceneFrame`]) and the surface
//! handle a [`RenderCommand::SurfaceCreated`] carries are therefore *generic*
//! parameters (`S`/`W`): a shell instantiates `S = frust_scene::Scene` and `W`
//! = its own raw-window wrapper, while these host tests instantiate cheap
//! stand-ins, so the whole channel is exercised without a GPU or a platform.
//!
//! # Wiring
//!
//! This module ships the channel types + pure logic; the desktop, Android,
//! and iOS shells each spawn their own render thread on top of it, gated by
//! [`render_thread_enabled`].
//!
//! [`Scene`]: https://docs.rs/frust-scene
//! [`UiSpans`]: crate::perf::UiSpans
//! [`RenderSpans`]: crate::perf::RenderSpans

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use frust_core::anim::FrameTime;

use crate::perf::UiSpans;

// ---------------------------------------------------------------------
// Kill switch
// ---------------------------------------------------------------------

/// The render-thread-split kill-switch environment/compile-time variable: when
/// set to any non-`"0"` value, [`render_thread_enabled`] is `false` and a shell
/// keeps the pre-split single-thread frame path.
///
/// Parsed exactly like [`crate::frame_gate::NO_FRAME_GATE_VAR`] — either the
/// compile-time `--define` or the runtime process env engages it.
pub const NO_RENDER_THREAD_VAR: &str = "FRUST_NO_RENDER_THREAD";

/// Whether a shell should run the render-thread split — the **single switch**
/// every shell consults. `true` unless the
/// [`NO_RENDER_THREAD_VAR`] kill switch is engaged (compile-time define or
/// runtime env, any non-`"0"` value), mirroring [`crate::perf::enabled`]'s and
/// [`crate::frame_gate`]'s `option_env!` + runtime-env parsing precedent.
///
/// Read once at shell startup: a shell that takes the split path spawns the
/// render thread, a shell where this is `false` keeps the single-thread path
/// verbatim.
pub fn render_thread_enabled() -> bool {
    !render_thread_kill_switch(
        option_env!("FRUST_NO_RENDER_THREAD"),
        std::env::var(NO_RENDER_THREAD_VAR).ok().as_deref(),
    )
}

/// The pure decision [`render_thread_enabled`] negates: a non-empty, non-`"0"`
/// value from either the compile-time or runtime source engages the kill
/// switch. Split out so it is directly unit-testable without touching the
/// process environment (see [`crate::frame_gate`]'s `kill_switch`).
fn render_thread_kill_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

// ---------------------------------------------------------------------
// Frame payload
// ---------------------------------------------------------------------

/// The surface dimensions a [`SceneFrame`] / [`RenderCommand`] carries — the
/// physical (device-pixel) swapchain size plus the HiDPI scale factor, so the
/// render thread can (re)configure the surface without consulting the UI
/// thread. Physical-at-the-boundary matches the render thread's swapchain
/// needs (`docs/CODE_STANDARDS.md`'s physical-at-FFI, logical-inside rule).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSize {
    /// Physical (device-pixel) width of the surface.
    pub width: u32,
    /// Physical (device-pixel) height of the surface.
    pub height: u32,
    /// HiDPI scale factor (physical / logical), already sanitized by the
    /// shell's [`sanitize_scale`](crate::sanitize_scale) at the FFI boundary.
    pub scale: f64,
}

/// Per-frame metadata riding the scene-handoff channel alongside the scene
/// itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMeta {
    /// The shell's frame clock for this frame (`Choreographer`/`CADisplayLink`/
    /// desktop epoch), threaded through to `PaintCtx::frame_time` so the render
    /// thread advances animations against the same clock the UI thread painted
    /// with. Only *differences* of two [`FrameTime`]s carry meaning (see its
    /// docs).
    pub frame_time: FrameTime,
    /// The surface size this scene was laid out for — the render thread checks
    /// it against the live swapchain configuration before encoding.
    pub size: SurfaceSize,
    /// Monotonically increasing per-frame id the UI thread stamps, so a dropped
    /// (latest-wins-replaced) frame is observable in diagnostics and so a
    /// render-side report can be paired back to the frame that produced it.
    pub frame_id: u64,
}

/// One frame handed from the UI thread to the render thread across
/// [`render_channel`]: the finished scene, its
/// [`FrameMeta`], and the UI thread's [`UiSpans`] half of the frame timing
/// (the render thread is the single perf emitter).
///
/// Generic over the scene type `S` so this crate stays render-free: a shell
/// instantiates `SceneFrame<frust_scene::Scene>`, host tests use a cheap
/// stand-in.
#[derive(Debug, Clone)]
pub struct SceneFrame<S> {
    /// The finished scene the render thread encodes (`frust_scene::Scene` in a
    /// real shell — `Scene` is `Send`, verified by its compile-time tripwire).
    pub scene: S,
    /// This frame's metadata (clock, surface size, id).
    pub meta: FrameMeta,
    /// The UI thread's `rebuild`/`layout`/`paint` timing, folded with the
    /// render thread's [`RenderSpans`](crate::perf::RenderSpans) via
    /// [`FramePasses::from_split`](crate::perf::FramePasses::from_split) into
    /// the one recorded frame.
    pub ui_spans: UiSpans,
}

// ---------------------------------------------------------------------
// Lifecycle: phase machine (modelled on frust-render's SurfacePhase)
// ---------------------------------------------------------------------

/// A surface-lifecycle event that drives a render-thread [`RenderPhase`]
/// transition — the pure, `Copy` counterpart of a [`RenderCommand`] (mirroring
/// `frust-render`'s `SurfaceEvent`/callback split, keeping the transition table
/// host-testable without the owned `Ack`/window payloads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderEvent {
    /// A surface became available (`surfaceCreated`/`resumed`/`set_surface`).
    SurfaceCreated,
    /// The existing surface was resized/reconfigured (rotation, inset change).
    SurfaceChanged,
    /// The surface is being torn down (`surfaceDestroyed`/`suspended`).
    SurfaceDestroyed,
    /// The app is backgrounding: stop submitting until [`Self::Resume`].
    Pause,
    /// The app returned to the foreground with its surface intact.
    Resume,
}

/// The render thread's view of surface lifecycle state,
/// modelled on `frust-render`'s `SurfacePhase`: the render loop renders a
/// handed-off [`SceneFrame`] only while [`can_render`](Self::can_render) — i.e.
/// only in [`RenderPhase::Active`]. [`RenderPhase::Paused`] is the cross-thread
/// backgrounding barrier (a leftover scene must NOT be submitted after a
/// `Pause`, per the iOS process-kill hazard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPhase {
    /// No usable surface: nothing to render into (initial state, or after a
    /// [`RenderEvent::SurfaceDestroyed`]).
    NoSurface,
    /// A configured surface is available and frames may be submitted.
    Active,
    /// The app backgrounded ([`RenderEvent::Pause`]); the surface may still
    /// exist but the render thread must not submit until [`RenderEvent::Resume`].
    Paused,
}

impl RenderPhase {
    /// Whether the render thread may submit a frame in this phase — only
    /// [`RenderPhase::Active`], mirroring `SurfacePhase::can_render`. A render
    /// loop checks this before encoding a handed-off [`SceneFrame`], so a scene
    /// left in the latest-wins slot when a `Pause`/`Destroy` is processed is
    /// dropped rather than submitted.
    pub fn can_render(self) -> bool {
        matches!(self, RenderPhase::Active)
    }
}

/// Pure render-phase transition table, the analogue of
/// `frust-render`'s `next_phase`. Total by design:
///
/// - `SurfaceCreated` → [`Active`](RenderPhase::Active) (create or recreate),
/// - `SurfaceDestroyed` → [`NoSurface`](RenderPhase::NoSurface),
/// - `SurfaceChanged` → the current phase unchanged (a resize never changes
///   *whether* we can render — mirrors "resize stays Ready"),
/// - `Pause` → [`Paused`](RenderPhase::Paused) unless there is no surface (you
///   cannot pause what was never created),
/// - `Resume` → [`Active`](RenderPhase::Active) unless there is no surface (you
///   cannot resume onto a surface that is gone; the shell must recreate it
///   first via `SurfaceCreated`).
pub fn next_render_phase(current: RenderPhase, event: RenderEvent) -> RenderPhase {
    match event {
        RenderEvent::SurfaceCreated => RenderPhase::Active,
        RenderEvent::SurfaceDestroyed => RenderPhase::NoSurface,
        RenderEvent::SurfaceChanged => current,
        RenderEvent::Pause => match current {
            RenderPhase::NoSurface => RenderPhase::NoSurface,
            _ => RenderPhase::Paused,
        },
        RenderEvent::Resume => match current {
            RenderPhase::NoSurface => RenderPhase::NoSurface,
            _ => RenderPhase::Active,
        },
    }
}

// ---------------------------------------------------------------------
// Lifecycle: acknowledgment barrier
// ---------------------------------------------------------------------

/// The render-thread side of an acknowledgment barrier: the render thread holds
/// this (moved out of a [`RenderCommand::Pause`]/[`RenderCommand::SurfaceDestroyed`])
/// while honoring the command, then [`acknowledge`](Self::acknowledge)s it —
/// unblocking the UI thread's paired [`AckWaiter`].
///
/// Dropping an `Ack` without an explicit [`acknowledge`](Self::acknowledge)
/// still signals (a safety net so a render thread that returns early — or
/// panics past the command — can never deadlock the UI thread), but a render
/// loop should acknowledge explicitly *after* the pause/destroy work is done,
/// which is exactly the barrier the two platform hazards need (Android window
/// release, iOS backgrounding).
#[derive(Debug)]
pub struct Ack {
    shared: Arc<AckShared>,
}

/// The UI-thread side of an acknowledgment barrier: the UI thread
/// [`wait`](Self::wait)s on this after sending a [`RenderCommand::Pause`]/
/// [`RenderCommand::SurfaceDestroyed`], blocking until the render thread has
/// [`acknowledge`](Ack::acknowledge)d (or dropped) the paired [`Ack`].
#[derive(Debug)]
pub struct AckWaiter {
    shared: Arc<AckShared>,
}

#[derive(Debug)]
struct AckShared {
    done: Mutex<bool>,
    signal: Condvar,
}

/// Create a linked [`AckWaiter`] / [`Ack`] barrier pair: the UI thread keeps
/// the waiter, the render thread receives the ack (inside the command). Used by
/// [`RenderSender::pause`]/[`RenderSender::destroy_surface`]; exposed directly
/// for shells building lifecycle commands by hand.
pub fn ack_pair() -> (AckWaiter, Ack) {
    let shared = Arc::new(AckShared {
        done: Mutex::new(false),
        signal: Condvar::new(),
    });
    (
        AckWaiter {
            shared: shared.clone(),
        },
        Ack { shared },
    )
}

impl Ack {
    /// Signal the paired [`AckWaiter`] that the command has been honored,
    /// consuming the ack. Equivalent to dropping it (the signal fires in
    /// [`Drop`]), but reads as the deliberate end-of-command acknowledgment the
    /// barrier contract expects.
    pub fn acknowledge(self) {
        // The Drop impl performs the signal; consuming `self` here runs it.
    }
}

impl Drop for Ack {
    fn drop(&mut self) {
        let mut done = self.shared.done.lock().unwrap();
        *done = true;
        drop(done);
        self.shared.signal.notify_all();
    }
}

impl AckWaiter {
    /// Block the UI thread until the render thread has acknowledged (or dropped)
    /// the paired [`Ack`]. Returns immediately if already acknowledged. This is
    /// the barrier: after it returns, the caller may safely proceed to release
    /// the window (Android) or let the app background (iOS).
    pub fn wait(self) {
        let mut done = self.shared.done.lock().unwrap();
        while !*done {
            done = self.shared.signal.wait(done).unwrap();
        }
    }

    /// Block the UI thread until the render thread acknowledges the paired
    /// [`Ack`] **or** `timeout` elapses, whichever comes first. Returns `true` if
    /// the ack fired (the barrier was honored), `false` on timeout.
    ///
    /// This is the **bounded** counterpart of [`Self::wait`]: the correctness fix
    /// (receiver-liveness — see [`RenderReceiver`]'s [`Drop`]) means a live render
    /// thread's ack always fires, but a render thread wedged mid-command (a GPU
    /// driver hang, not a clean exit) would still block [`Self::wait`] forever.
    /// A timeout lets the caller **degrade instead of hang** — proceed to release
    /// the window / let the app background after logging — so a stuck render
    /// thread never trips a platform watchdog (iOS backgrounding kill, Android
    /// ANR). Each call site picks a named, doc-commented per-platform deadline
    /// safely under its watchdog budget.
    #[must_use = "the caller must handle a timeout (proceed degraded) rather than assume the barrier was honored"]
    pub fn wait_timeout(self, timeout: Duration) -> bool {
        let done = self.shared.done.lock().unwrap();
        let (done, _timeout_result) = self
            .shared
            .signal
            .wait_timeout_while(done, timeout, |done| !*done)
            .unwrap();
        *done
    }

    /// Whether the paired [`Ack`] has been acknowledged yet, without blocking —
    /// a non-consuming diagnostic peek (the barrier proper is [`Self::wait`]).
    pub fn completed(&self) -> bool {
        *self.shared.done.lock().unwrap()
    }
}

// ---------------------------------------------------------------------
// Lifecycle: command vocabulary
// ---------------------------------------------------------------------

/// A lifecycle command the UI thread sends to the render thread across
/// [`render_channel`], as an **owned** value — not a shared
/// mutable flag. Generic over the surface-handle type `W` a
/// [`Self::SurfaceCreated`] carries (`frust-render`'s raw-window wrapper in a
/// real shell; a stand-in in host tests), keeping this crate render-free.
///
/// [`Self::Pause`] and [`Self::SurfaceDestroyed`] carry an [`Ack`]: the UI
/// thread blocks on the paired [`AckWaiter`] until the render thread honors
/// them (the window-release / backgrounding barrier). The others are
/// fire-and-forget. The command's effect on the render thread's [`RenderPhase`]
/// is given by [`Self::event`] → [`next_render_phase`].
#[derive(Debug)]
pub enum RenderCommand<W> {
    /// A surface became available: the render thread takes ownership of `window`
    /// and (re)configures its swapchain to `size`. Fire-and-forget.
    SurfaceCreated {
        /// The raw surface handle the render thread takes ownership of.
        window: W,
        /// The initial physical surface size.
        size: SurfaceSize,
    },
    /// The existing surface was resized/reconfigured (rotation, inset change).
    /// Fire-and-forget.
    SurfaceChanged {
        /// The new physical surface size.
        size: SurfaceSize,
    },
    /// The surface is being torn down: the render thread must drop every
    /// surface-derived `wgpu` resource **before acknowledging**, so the UI
    /// thread can safely release the underlying window (the Android
    /// `ANativeWindow`-release hazard). Blocks the UI thread via [`Ack`].
    SurfaceDestroyed {
        /// Acknowledged once surface resources are dropped.
        ack: Ack,
    },
    /// The app is backgrounding: the render thread must stop submitting frames
    /// **before acknowledging**, so the app never submits Metal/Vulkan work
    /// after it backgrounds (the iOS process-kill hazard). Blocks the UI thread
    /// via [`Ack`].
    Pause {
        /// Acknowledged once the render thread has quiesced.
        ack: Ack,
    },
    /// The app returned to the foreground with its surface intact: resume
    /// submitting. Fire-and-forget.
    Resume,
}

impl<W> RenderCommand<W> {
    /// The pure [`RenderEvent`] this command drives on the render thread's
    /// [`RenderPhase`] — the `Copy` projection that feeds [`next_render_phase`]
    /// (borrowing `self`, leaving the owned `Ack`/`window` in place).
    pub fn event(&self) -> RenderEvent {
        match self {
            RenderCommand::SurfaceCreated { .. } => RenderEvent::SurfaceCreated,
            RenderCommand::SurfaceChanged { .. } => RenderEvent::SurfaceChanged,
            RenderCommand::SurfaceDestroyed { .. } => RenderEvent::SurfaceDestroyed,
            RenderCommand::Pause { .. } => RenderEvent::Pause,
            RenderCommand::Resume => RenderEvent::Resume,
        }
    }

    /// Whether this command carries an [`Ack`] the UI thread blocks on — `true`
    /// for [`Self::Pause`]/[`Self::SurfaceDestroyed`], `false` for the
    /// fire-and-forget variants.
    pub fn requires_ack(&self) -> bool {
        matches!(
            self,
            RenderCommand::SurfaceDestroyed { .. } | RenderCommand::Pause { .. }
        )
    }
}

// ---------------------------------------------------------------------
// The UI→render channel: latest-wins scene slot + FIFO command queue
// ---------------------------------------------------------------------

/// The shared state behind [`render_channel`]: a depth-1 latest-wins scene slot
/// and a FIFO command queue, both under one mutex + condvar so the render
/// thread has a single wait point.
#[derive(Debug)]
struct Inbox<S, W> {
    /// The freshest un-taken scene (depth-1 latest-wins): a newer send replaces
    /// it, incrementing [`Self::dropped`].
    latest: Option<SceneFrame<S>>,
    /// Count of scenes replaced (dropped) before the render thread took them —
    /// the latest-wins drop counter, observable via
    /// [`RenderReceiver::dropped_frames`].
    dropped: u64,
    /// Pending lifecycle commands in FIFO order.
    commands: Vec<RenderCommand<W>>,
    /// Cleared when the [`RenderSender`] is dropped, so a blocked
    /// [`RenderReceiver::wait_next`] wakes and reports disconnection (the render
    /// loop's clean-exit signal).
    sender_alive: bool,
    /// Cleared when the [`RenderReceiver`] is dropped (the render thread exited —
    /// panic-unwind, a clean early `return`, or a hung thread's drop). Once
    /// `false`, [`RenderSender::send_command`]/[`RenderSender::send_scene`] drop
    /// (rather than queue) new work: an ack-carrying command dropped here fires
    /// its [`Ack`]'s [`Drop`] safety net, so a UI thread blocked on the paired
    /// [`AckWaiter`] can never wedge on a command the departed render thread will
    /// never drain. Symmetric with [`Self::sender_alive`].
    receiver_alive: bool,
}

#[derive(Debug)]
struct Channel<S, W> {
    inbox: Mutex<Inbox<S, W>>,
    signal: Condvar,
}

/// The UI-thread handle to the render channel: sends scenes
/// (latest-wins) and lifecycle commands (FIFO). Single-producer by design (the
/// UI thread), so it is deliberately not [`Clone`].
#[derive(Debug)]
pub struct RenderSender<S, W> {
    channel: Arc<Channel<S, W>>,
}

/// The render-thread handle to the render channel: the
/// single wait point ([`Self::wait_next`]) draining pending commands plus the
/// freshest scene each wakeup.
#[derive(Debug)]
pub struct RenderReceiver<S, W> {
    channel: Arc<Channel<S, W>>,
}

/// One wakeup's worth of work handed to the render thread by
/// [`RenderReceiver::wait_next`]/[`RenderReceiver::try_next`]: the lifecycle
/// commands to process (FIFO), then the freshest scene to render (if any). A
/// render loop processes `commands` first (updating its [`RenderPhase`]), then
/// renders `scene` only if the resulting phase [`can_render`](RenderPhase::can_render).
#[derive(Debug)]
pub struct RenderBatch<S, W> {
    /// Pending lifecycle commands in FIFO order.
    pub commands: Vec<RenderCommand<W>>,
    /// The freshest scene handed off since the last drain (latest-wins), or
    /// `None` if no new scene arrived.
    pub scene: Option<SceneFrame<S>>,
    /// `true` once the [`RenderSender`] has been dropped and no work remains —
    /// the render loop's signal to exit cleanly.
    pub disconnected: bool,
}

/// Create the UI→render channel: a depth-1 latest-wins
/// scene slot fused with a FIFO lifecycle-command queue behind one condvar.
///
/// `S` is the scene payload type (`frust_scene::Scene` in a real shell), `W`
/// the surface-handle type a [`RenderCommand::SurfaceCreated`] carries — both
/// generic so this crate stays render-free (see the module docs' Layering
/// choice).
pub fn render_channel<S, W>() -> (RenderSender<S, W>, RenderReceiver<S, W>) {
    let channel = Arc::new(Channel {
        inbox: Mutex::new(Inbox {
            latest: None,
            dropped: 0,
            commands: Vec::new(),
            sender_alive: true,
            receiver_alive: true,
        }),
        signal: Condvar::new(),
    });
    (
        RenderSender {
            channel: channel.clone(),
        },
        RenderReceiver { channel },
    )
}

impl<S, W> RenderSender<S, W> {
    /// Hand a finished frame to the render thread (**depth-1 latest-wins**): if
    /// an un-taken scene is still in the slot it is replaced (the drop counter
    /// still increments — see [`Inbox::dropped`]) and **returned** to the
    /// caller instead of being silently dropped in the lock — a shell can
    /// reclaim the stale frame's scene buffer the same way
    /// it reclaims one off [`scene_return_channel`]. `None` if the slot was
    /// empty. A pure widening of the original fire-and-forget signature — a
    /// caller that doesn't care may still ignore the return value. Wakes the
    /// render thread's [`RenderReceiver::wait_next`].
    pub fn send_scene(&self, frame: SceneFrame<S>) -> Option<SceneFrame<S>> {
        let mut inbox = self.channel.inbox.lock().unwrap();
        if !inbox.receiver_alive {
            // The render thread is gone (see `Inbox::receiver_alive`): don't
            // queue `frame` into a slot no one will ever take — hand it straight
            // back so the caller can still reclaim its buffer. `frame` carries no
            // `Ack`, so nothing else needs firing either way.
            return Some(frame);
        }
        let stale = inbox.latest.replace(frame);
        if stale.is_some() {
            inbox.dropped += 1;
        }
        drop(inbox);
        self.channel.signal.notify_one();
        stale
    }

    /// Queue a lifecycle command (FIFO) and wake the render thread. For the
    /// ack-carrying [`RenderCommand::Pause`]/[`RenderCommand::SurfaceDestroyed`]
    /// prefer [`Self::pause`]/[`Self::destroy_surface`], which build the barrier
    /// pair and return the [`AckWaiter`] to block on.
    pub fn send_command(&self, command: RenderCommand<W>) {
        let mut inbox = self.channel.inbox.lock().unwrap();
        if !inbox.receiver_alive {
            // The render thread is gone (see `Inbox::receiver_alive`): drop the
            // command rather than queue it forever. Releasing the inbox lock first,
            // then dropping `command`, fires any embedded `Ack`'s `Drop` safety net
            // (Pause/SurfaceDestroyed), so a UI thread blocked on the paired
            // `AckWaiter` unblocks instead of deadlocking.
            drop(inbox);
            drop(command);
            return;
        }
        inbox.commands.push(command);
        drop(inbox);
        self.channel.signal.notify_one();
    }

    /// Send a [`RenderCommand::Pause`] and return the [`AckWaiter`] the UI
    /// thread must [`wait`](AckWaiter::wait) on **before letting the app
    /// background** — the iOS process-kill barrier.
    #[must_use = "the caller must wait() on the returned AckWaiter before backgrounding"]
    pub fn pause(&self) -> AckWaiter {
        let (waiter, ack) = ack_pair();
        self.send_command(RenderCommand::Pause { ack });
        waiter
    }

    /// Send a [`RenderCommand::SurfaceDestroyed`] and return the [`AckWaiter`]
    /// the UI thread must [`wait`](AckWaiter::wait) on **before releasing the
    /// window** — the Android `ANativeWindow`-release barrier.
    #[must_use = "the caller must wait() on the returned AckWaiter before releasing the window"]
    pub fn destroy_surface(&self) -> AckWaiter {
        let (waiter, ack) = ack_pair();
        self.send_command(RenderCommand::SurfaceDestroyed { ack });
        waiter
    }
}

impl<S, W> Drop for RenderSender<S, W> {
    fn drop(&mut self) {
        let mut inbox = self.channel.inbox.lock().unwrap();
        inbox.sender_alive = false;
        drop(inbox);
        // notify_all: a receiver blocked in wait_next must wake to observe the
        // disconnection and exit its loop.
        self.channel.signal.notify_all();
    }
}

impl<S, W> Drop for RenderReceiver<S, W> {
    fn drop(&mut self) {
        // The render thread is exiting (panic-unwind, a clean early `return`, or a
        // hung thread being torn down). Symmetric with `Drop for RenderSender`:
        // mark the receiver gone and drain any undrained work so an ack-carrying
        // command the render loop never reached (a `Pause`/`SurfaceDestroyed`
        // still in `commands`, or embedded in `latest` — the latter carries none
        // today, drained for completeness) fires its `Ack`'s `Drop` safety net.
        // Without this, that command would sit in the inbox forever (kept alive by
        // the `Arc<Channel>` the still-blocked UI side holds), the safety net would
        // never fire, and `AckWaiter::wait()` would deadlock the UI/main thread —
        // the load-bearing correctness fix this drop impl provides.
        let mut inbox = self.channel.inbox.lock().unwrap();
        inbox.receiver_alive = false;
        let commands = std::mem::take(&mut inbox.commands);
        let latest = inbox.latest.take();
        drop(inbox);
        // Drop the drained work *after* releasing the inbox lock — dropping an
        // `Ack` locks its own (separate) mutex to signal, so ordering here avoids
        // holding the inbox lock across that notify.
        drop(commands);
        drop(latest);
        // notify_all for symmetry with the sender's drop (no thread blocks on the
        // channel condvar once the receiver is gone, but a stray waiter must never
        // be left parked).
        self.channel.signal.notify_all();
    }
}

impl<S, W> RenderReceiver<S, W> {
    /// Block until there is work — a pending command, a fresh scene, or a
    /// [`RenderSender`] disconnection — then drain it into one [`RenderBatch`].
    /// The render thread's single wait point.
    pub fn wait_next(&self) -> RenderBatch<S, W> {
        let mut inbox = self.channel.inbox.lock().unwrap();
        inbox = self
            .channel
            .signal
            .wait_while(inbox, |i| {
                i.commands.is_empty() && i.latest.is_none() && i.sender_alive
            })
            .unwrap();
        drain(&mut inbox)
    }

    /// Drain any pending commands + the freshest scene without blocking — for a
    /// render loop that also polls its own timers/vsync. Returns an empty,
    /// non-disconnected batch when nothing is pending and the sender is alive.
    pub fn try_next(&self) -> RenderBatch<S, W> {
        let mut inbox = self.channel.inbox.lock().unwrap();
        drain(&mut inbox)
    }

    /// The running count of scenes dropped by the latest-wins slot (replaced
    /// before the render thread took them) — a diagnostic for how far the
    /// render thread is falling behind the UI thread.
    pub fn dropped_frames(&self) -> u64 {
        self.channel.inbox.lock().unwrap().dropped
    }
}

/// Drain the inbox into a [`RenderBatch`]: all pending commands (FIFO), the
/// freshest scene (latest-wins), and the disconnection flag. Shared by
/// [`RenderReceiver::wait_next`]/[`RenderReceiver::try_next`].
fn drain<S, W>(inbox: &mut Inbox<S, W>) -> RenderBatch<S, W> {
    RenderBatch {
        commands: std::mem::take(&mut inbox.commands),
        scene: inbox.latest.take(),
        disconnected: !inbox.sender_alive,
    }
}

// ---------------------------------------------------------------------
// Scene give-back: a non-blocking depth-1 return slot
// ---------------------------------------------------------------------

/// The render-thread handle to [`scene_return_channel`]: pushes a drained
/// scene back for the UI thread to reclaim (`Scene::reset` + reuse) instead
/// of a shell allocating a fresh one every frame — closing the buffer-reuse
/// gap a scene crossing [`render_channel`] would otherwise leave (a scene
/// with no way back, so every split `submit_frame` replaced it with
/// `Scene::new()`).
#[derive(Debug)]
pub struct SceneReturnSender<S> {
    slot: Arc<Mutex<Option<S>>>,
}

/// The UI-thread handle to [`scene_return_channel`]: polls (never blocks)
/// for a scene the render thread has finished with.
#[derive(Debug)]
pub struct SceneReturnReceiver<S> {
    slot: Arc<Mutex<Option<S>>>,
}

/// Create the render→UI scene give-back channel: a
/// non-blocking, depth-1 return slot — the reverse-direction, pull-based
/// counterpart to [`render_channel`]'s UI→render handoff. `Mutex<Option<S>>`
/// only, no [`Condvar`] and no new dependency: nothing should ever park
/// waiting on this slot, so there is no wait point to back — preserving this
/// module's no-`unsafe`, no-new-deps, generic-over-`S` charter (see the
/// module docs' Layering choice).
pub fn scene_return_channel<S>() -> (SceneReturnSender<S>, SceneReturnReceiver<S>) {
    let slot = Arc::new(Mutex::new(None));
    (
        SceneReturnSender { slot: slot.clone() },
        SceneReturnReceiver { slot },
    )
}

impl<S> SceneReturnSender<S> {
    /// Push a drained scene back for the UI thread to reclaim. Depth-1
    /// latest-wins, mirroring [`RenderSender::send_scene`]: an unpolled scene
    /// already in the slot is replaced (dropped) rather than queued — the UI
    /// thread only ever needs one spare, and an unbounded backlog here would
    /// just be a leak-shaped wait for a UI thread that has stopped polling.
    ///
    /// The render thread must call this for **every** [`SceneFrame`] it takes
    /// off a [`RenderReceiver`] — whether the scene is actually rendered or
    /// the frame is phase-gated out ([`RenderPhase::Paused`]/[`RenderPhase::NoSurface`]
    /// after a `Pause`/`SurfaceDestroyed`) — so a scene is never silently
    /// dropped instead of given back.
    pub fn give_back(&self, scene: S) {
        let mut slot = self.slot.lock().unwrap();
        *slot = Some(scene);
    }
}

impl<S> SceneReturnReceiver<S> {
    /// Non-blocking poll for a returned scene — `None` if the render thread
    /// hasn't given one back yet (cold start, or it is still busy on the
    /// current frame). Never blocks: a caller that finds nothing falls back
    /// to allocating a fresh scene (`Scene::new()`).
    pub fn try_recv(&self) -> Option<S> {
        self.slot.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    // -----------------------------------------------------------------
    // Kill switch (pure — the env-reading wrapper's cache-free counterpart,
    // mirroring frame_gate's kill_switch tests)
    // -----------------------------------------------------------------

    #[test]
    fn render_thread_kill_switch_off_when_neither_set() {
        assert!(!render_thread_kill_switch(None, None));
    }

    #[test]
    fn render_thread_kill_switch_on_when_compile_time_set_non_zero() {
        assert!(render_thread_kill_switch(Some("1"), None));
    }

    #[test]
    fn render_thread_kill_switch_on_when_runtime_set_non_zero() {
        assert!(render_thread_kill_switch(None, Some("1")));
    }

    #[test]
    fn render_thread_kill_switch_off_when_either_is_literal_zero_and_other_unset() {
        assert!(!render_thread_kill_switch(Some("0"), None));
        assert!(!render_thread_kill_switch(None, Some("0")));
    }

    #[test]
    fn render_thread_kill_switch_on_when_either_source_wins() {
        assert!(render_thread_kill_switch(Some("0"), Some("1")));
        assert!(render_thread_kill_switch(Some("1"), Some("0")));
    }

    #[test]
    fn render_thread_enabled_default_process_env_is_on() {
        // A normal test run sets neither the compile-time define nor the runtime
        // env, so the split is enabled by default (the kill switch is the
        // opt-out, mirroring frame_gate). This is the "kill-switch off path".
        assert!(render_thread_enabled());
    }

    // -----------------------------------------------------------------
    // RenderPhase transition table (modelled on frust-render's SurfacePhase)
    // -----------------------------------------------------------------

    const ALL_PHASES: [RenderPhase; 3] = [
        RenderPhase::NoSurface,
        RenderPhase::Active,
        RenderPhase::Paused,
    ];

    #[test]
    fn created_always_lands_in_active() {
        for phase in ALL_PHASES {
            assert_eq!(
                next_render_phase(phase, RenderEvent::SurfaceCreated),
                RenderPhase::Active,
                "creating (or recreating) a surface must reach Active from {phase:?}"
            );
        }
    }

    #[test]
    fn destroyed_always_lands_in_no_surface() {
        for phase in ALL_PHASES {
            assert_eq!(
                next_render_phase(phase, RenderEvent::SurfaceDestroyed),
                RenderPhase::NoSurface,
                "destroying a surface must reach NoSurface from {phase:?}"
            );
        }
    }

    #[test]
    fn changed_keeps_the_current_phase() {
        for phase in ALL_PHASES {
            assert_eq!(
                next_render_phase(phase, RenderEvent::SurfaceChanged),
                phase,
                "a resize must not change whether we can render, from {phase:?}"
            );
        }
    }

    #[test]
    fn pause_barriers_from_active_but_not_from_no_surface() {
        assert_eq!(
            next_render_phase(RenderPhase::Active, RenderEvent::Pause),
            RenderPhase::Paused
        );
        assert_eq!(
            next_render_phase(RenderPhase::Paused, RenderEvent::Pause),
            RenderPhase::Paused
        );
        // You cannot pause what was never created.
        assert_eq!(
            next_render_phase(RenderPhase::NoSurface, RenderEvent::Pause),
            RenderPhase::NoSurface
        );
    }

    #[test]
    fn resume_reactivates_unless_the_surface_is_gone() {
        assert_eq!(
            next_render_phase(RenderPhase::Paused, RenderEvent::Resume),
            RenderPhase::Active
        );
        assert_eq!(
            next_render_phase(RenderPhase::Active, RenderEvent::Resume),
            RenderPhase::Active
        );
        // Cannot resume onto a surface that is gone — must be recreated first.
        assert_eq!(
            next_render_phase(RenderPhase::NoSurface, RenderEvent::Resume),
            RenderPhase::NoSurface
        );
    }

    #[test]
    fn only_active_can_render() {
        assert!(RenderPhase::Active.can_render());
        assert!(!RenderPhase::NoSurface.can_render());
        assert!(!RenderPhase::Paused.can_render());
    }

    // -----------------------------------------------------------------
    // RenderCommand event/ack projections
    // -----------------------------------------------------------------

    #[test]
    fn command_event_projection_matches_each_variant() {
        // `()` for both the scene and window type params — a cheap host stand-in.
        let created: RenderCommand<()> = RenderCommand::SurfaceCreated {
            window: (),
            size: test_size(),
        };
        assert_eq!(created.event(), RenderEvent::SurfaceCreated);
        assert!(!created.requires_ack());

        let changed: RenderCommand<()> = RenderCommand::SurfaceChanged { size: test_size() };
        assert_eq!(changed.event(), RenderEvent::SurfaceChanged);
        assert!(!changed.requires_ack());

        let resume: RenderCommand<()> = RenderCommand::Resume;
        assert_eq!(resume.event(), RenderEvent::Resume);
        assert!(!resume.requires_ack());

        let (_w, ack) = ack_pair();
        let paused: RenderCommand<()> = RenderCommand::Pause { ack };
        assert_eq!(paused.event(), RenderEvent::Pause);
        assert!(paused.requires_ack());

        let (_w, ack) = ack_pair();
        let destroyed: RenderCommand<()> = RenderCommand::SurfaceDestroyed { ack };
        assert_eq!(destroyed.event(), RenderEvent::SurfaceDestroyed);
        assert!(destroyed.requires_ack());
    }

    // -----------------------------------------------------------------
    // Scene handoff: depth-1 latest-wins semantics
    // -----------------------------------------------------------------

    fn test_size() -> SurfaceSize {
        SurfaceSize {
            width: 1080,
            height: 1920,
            scale: 2.0,
        }
    }

    /// A scene frame carrying a `u32` scene id — a cheap host stand-in for a
    /// real `frust_scene::Scene`.
    fn frame(id: u32) -> SceneFrame<u32> {
        SceneFrame {
            scene: id,
            meta: FrameMeta {
                frame_time: FrameTime::from_nanos(u64::from(id)),
                size: test_size(),
                frame_id: u64::from(id),
            },
            ui_spans: UiSpans::default(),
        }
    }

    #[test]
    fn latest_wins_replaces_an_untaken_scene() {
        let (tx, rx) = render_channel::<u32, ()>();
        tx.send_scene(frame(1));
        tx.send_scene(frame(2));
        tx.send_scene(frame(3));

        // Only the freshest survives; the two older frames were dropped.
        let batch = rx.try_next();
        assert_eq!(batch.scene.expect("a scene is pending").scene, 3);
        assert_eq!(rx.dropped_frames(), 2, "two stale frames were replaced");

        // The slot is now empty.
        let batch = rx.try_next();
        assert!(batch.scene.is_none(), "the slot is drained after a take");
    }

    #[test]
    fn taking_between_sends_drops_nothing() {
        let (tx, rx) = render_channel::<u32, ()>();
        tx.send_scene(frame(1));
        assert_eq!(rx.try_next().scene.unwrap().scene, 1);
        tx.send_scene(frame(2));
        assert_eq!(rx.try_next().scene.unwrap().scene, 2);
        assert_eq!(
            rx.dropped_frames(),
            0,
            "each scene was taken before the next"
        );
    }

    #[test]
    fn send_scene_returns_none_when_the_slot_was_empty() {
        let (tx, _rx) = render_channel::<u32, ()>();
        assert!(
            tx.send_scene(frame(1)).is_none(),
            "the first send has nothing stale to hand back"
        );
    }

    #[test]
    fn send_scene_returns_the_overwritten_stale_frame() {
        // An untaken scene replaced by a newer send must be
        // handed back to the caller (to reclaim its buffer), not silently
        // dropped in the lock.
        let (tx, rx) = render_channel::<u32, ()>();
        assert!(tx.send_scene(frame(1)).is_none());
        let stale = tx
            .send_scene(frame(2))
            .expect("the untaken frame(1) must be returned");
        assert_eq!(stale.scene, 1, "the returned frame is the one replaced");

        // Latest-wins semantics are unchanged by the give-back: the freshest
        // scene is still what the render thread takes, and the drop counter
        // still increments exactly as before.
        let batch = rx.try_next();
        assert_eq!(batch.scene.expect("a scene is pending").scene, 2);
        assert_eq!(rx.dropped_frames(), 1);
    }

    #[test]
    fn send_scene_after_receiver_death_hands_the_frame_back() {
        // The dead-receiver path (see `Inbox::receiver_alive`) must not queue
        // the frame, but should still let the caller reclaim its buffer rather
        // than drop it on the floor.
        let (tx, rx) = render_channel::<u32, ()>();
        drop(rx);
        let returned = tx
            .send_scene(frame(1))
            .expect("a scene sent after receiver death must still be handed back");
        assert_eq!(returned.scene, 1);
    }

    // -----------------------------------------------------------------
    // Scene give-back channel: non-blocking depth-1
    // return slot, render thread -> UI thread.
    // -----------------------------------------------------------------

    #[test]
    fn scene_return_try_recv_is_none_before_any_give_back() {
        let (_tx, rx) = scene_return_channel::<u32>();
        assert!(rx.try_recv().is_none());
    }

    #[test]
    fn scene_return_round_trips_a_single_scene() {
        let (tx, rx) = scene_return_channel::<u32>();
        tx.give_back(7);
        assert_eq!(rx.try_recv(), Some(7), "the given-back scene is polled out");
        assert!(
            rx.try_recv().is_none(),
            "the slot is drained after a take, like the forward channel"
        );
    }

    #[test]
    fn scene_return_is_depth_1_latest_wins() {
        // Mirrors `render_channel`'s forward-slot semantics: a second give-back
        // before the UI thread polls replaces (not queues) the first.
        let (tx, rx) = scene_return_channel::<u32>();
        tx.give_back(1);
        tx.give_back(2);
        assert_eq!(
            rx.try_recv(),
            Some(2),
            "only the most recently given-back scene survives"
        );
    }

    #[test]
    fn scene_return_never_blocks_the_ui_thread() {
        // The whole point of the non-blocking design: polling an empty slot
        // returns immediately rather than parking, even with no render-thread
        // counterpart ever constructed to give one back.
        let (_tx, rx) = scene_return_channel::<u32>();
        let start = Instant::now();
        assert!(rx.try_recv().is_none());
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "try_recv must return immediately, never park"
        );
    }

    #[test]
    fn wait_next_blocks_until_a_scene_arrives() {
        let (tx, rx) = render_channel::<u32, ()>();
        let handle = thread::spawn(move || rx.wait_next().scene.map(|f| f.scene));
        // Give the render thread a moment to park in wait_next, then hand it a
        // scene; the join proves it woke and took it.
        thread::sleep(Duration::from_millis(20));
        tx.send_scene(frame(7));
        assert_eq!(handle.join().unwrap(), Some(7));
    }

    // -----------------------------------------------------------------
    // Command ordering + disconnection
    // -----------------------------------------------------------------

    #[test]
    fn commands_drain_in_fifo_order_with_the_latest_scene() {
        let (tx, rx) = render_channel::<u32, ()>();
        tx.send_command(RenderCommand::SurfaceCreated {
            window: (),
            size: test_size(),
        });
        tx.send_scene(frame(1));
        tx.send_command(RenderCommand::SurfaceChanged { size: test_size() });
        tx.send_scene(frame(2)); // replaces frame(1)

        let batch = rx.try_next();
        let events: Vec<RenderEvent> = batch.commands.iter().map(RenderCommand::event).collect();
        assert_eq!(
            events,
            vec![RenderEvent::SurfaceCreated, RenderEvent::SurfaceChanged],
            "commands preserve FIFO order"
        );
        assert_eq!(
            batch.scene.unwrap().scene,
            2,
            "only the freshest scene rides along"
        );
        assert!(!batch.disconnected);
    }

    #[test]
    fn wait_next_wakes_and_reports_disconnection_when_sender_dropped() {
        let (tx, rx) = render_channel::<u32, ()>();
        let handle = thread::spawn(move || rx.wait_next().disconnected);
        thread::sleep(Duration::from_millis(20));
        drop(tx); // must wake the parked receiver
        assert!(
            handle.join().unwrap(),
            "dropping the sender wakes wait_next with a disconnection"
        );
    }

    #[test]
    fn pending_work_drains_before_disconnection_is_reported() {
        let (tx, rx) = render_channel::<u32, ()>();
        tx.send_scene(frame(5));
        drop(tx);
        // The buffered scene is still delivered; disconnected is also set so the
        // loop exits after handling it.
        let batch = rx.try_next();
        assert_eq!(batch.scene.unwrap().scene, 5);
        assert!(batch.disconnected);
    }

    // -----------------------------------------------------------------
    // Acknowledgment barrier (pause / destroy)
    // -----------------------------------------------------------------

    #[test]
    fn ack_unblocks_the_waiter_on_acknowledge() {
        let (waiter, ack) = ack_pair();
        assert!(!waiter.completed());
        ack.acknowledge();
        assert!(waiter.completed());
        waiter.wait(); // returns immediately
    }

    #[test]
    fn ack_unblocks_the_waiter_on_drop_as_a_safety_net() {
        let (waiter, ack) = ack_pair();
        drop(ack); // a render thread that returns early must not deadlock the UI
        assert!(waiter.completed());
    }

    #[test]
    fn pause_barrier_blocks_the_ui_thread_until_the_render_thread_quiesces() {
        // The plan's iOS backgrounding contract: the UI thread must not proceed
        // (let the app background) until the render thread has honored Pause.
        let (tx, rx) = render_channel::<u32, ()>();
        let quiesced = Arc::new(AtomicBool::new(false));
        let quiesced_render = quiesced.clone();

        let render = thread::spawn(move || {
            loop {
                let batch = rx.wait_next();
                for command in batch.commands {
                    if let RenderCommand::Pause { ack } = command {
                        // Simulate stopping submission *before* acknowledging.
                        thread::sleep(Duration::from_millis(30));
                        quiesced_render.store(true, Ordering::SeqCst);
                        ack.acknowledge();
                        return;
                    }
                }
                if batch.disconnected {
                    return;
                }
            }
        });

        let waiter = tx.pause();
        waiter.wait();
        // The barrier guarantees the render thread quiesced before wait returned.
        assert!(
            quiesced.load(Ordering::SeqCst),
            "the render thread must have quiesced before the UI thread proceeded"
        );
        render.join().unwrap();
    }

    #[test]
    fn destroy_surface_barrier_orders_resource_teardown_before_window_release() {
        // The plan's Android ANativeWindow-release contract: the UI thread must
        // not release the window until the render thread has dropped its
        // surface resources.
        let (tx, rx) = render_channel::<u32, ()>();
        let resources_dropped = Arc::new(AtomicBool::new(false));
        let resources_dropped_render = resources_dropped.clone();

        let render = thread::spawn(move || {
            loop {
                let batch = rx.wait_next();
                for command in batch.commands {
                    if let RenderCommand::SurfaceDestroyed { ack } = command {
                        thread::sleep(Duration::from_millis(30));
                        resources_dropped_render.store(true, Ordering::SeqCst);
                        ack.acknowledge();
                        return;
                    }
                }
                if batch.disconnected {
                    return;
                }
            }
        });

        let waiter = tx.destroy_surface();
        waiter.wait();
        assert!(
            resources_dropped.load(Ordering::SeqCst),
            "surface resources must be dropped before the UI thread releases the window"
        );
        render.join().unwrap();
    }

    #[test]
    fn a_leftover_scene_is_not_rendered_after_a_pause() {
        // The render loop gates on RenderPhase: a scene left in the slot when a
        // Pause is processed must be dropped, not submitted (the iOS
        // submit-after-background hazard). This exercises the phase-gating
        // composition the shells rely on.
        let (tx, rx) = render_channel::<u32, ()>();
        tx.send_scene(frame(1)); // a frame still in the slot...
        let _waiter = tx.pause(); // ...when a Pause arrives

        let mut phase = RenderPhase::Active;
        let mut submitted: Option<u32> = None;
        let batch = rx.try_next();
        for command in batch.commands {
            phase = next_render_phase(phase, command.event());
            if let RenderCommand::Pause { ack } = command {
                ack.acknowledge();
            }
        }
        if phase.can_render() {
            submitted = batch.scene.map(|f| f.scene);
        }
        assert_eq!(phase, RenderPhase::Paused);
        assert!(
            submitted.is_none(),
            "a leftover scene must not be submitted after a Pause"
        );
    }

    // -----------------------------------------------------------------
    // Receiver-liveness: a render thread that exits
    // before draining an ack-carrying command must never deadlock the UI
    // thread. Every assertion here is timeout-bounded so a *regression* FAILS
    // (the timeout expires, returning `false`) rather than hanging the suite.
    // -----------------------------------------------------------------

    /// A deadline generous enough that the correct path (the ack fires the
    /// instant the receiver drops) always beats it, yet finite so a regression
    /// fails the test instead of wedging the whole `cargo test` run.
    const REGRESSION_DEADLINE: Duration = Duration::from_secs(5);

    #[test]
    fn receiver_drop_drains_a_queued_orphaned_ack() {
        // The barrier command is queued while the receiver is still alive...
        let (tx, rx) = render_channel::<u32, ()>();
        let waiter = tx.pause();
        // ...then the render thread exits before draining it (drops its receiver).
        drop(rx);
        // The orphaned `Ack` must have fired via `RenderReceiver::drop`, so the
        // UI-side waiter unblocks rather than deadlocking.
        assert!(
            waiter.wait_timeout(REGRESSION_DEADLINE),
            "dropping the receiver must drain the queued Pause's Ack so the UI waiter unblocks"
        );
    }

    #[test]
    fn command_sent_after_receiver_death_fires_its_ack() {
        // The receiver is already gone before the barrier command is sent: the
        // sender must drop (not queue) it, still firing the embedded Ack.
        let (tx, rx) = render_channel::<u32, ()>();
        drop(rx);
        let waiter = tx.destroy_surface();
        assert!(
            waiter.wait_timeout(REGRESSION_DEADLINE),
            "an ack-carrying command sent after receiver death must fire its Ack safety net"
        );
    }

    #[test]
    fn receiver_death_mid_flight_unblocks_a_waiting_ui_thread() {
        // The closest reproduction of the live hazard: the UI thread is *already*
        // blocked on the barrier when the render thread dies. A background thread
        // sends the barrier and blocks on it (bounded); the main thread drops the
        // receiver a moment later, standing in for the render thread's exit.
        let (tx, rx) = render_channel::<u32, ()>();
        let (report_tx, report_rx) = std::sync::mpsc::channel();
        let ui = thread::spawn(move || {
            let honored = tx.destroy_surface().wait_timeout(REGRESSION_DEADLINE);
            report_tx.send(honored).unwrap();
        });
        thread::sleep(Duration::from_millis(20));
        drop(rx); // render thread exits without draining the command

        // The UI thread must have unblocked; a regression would leave it parked
        // until its own wait_timeout expired `false`.
        let honored = report_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the UI thread must report back, not stay deadlocked");
        assert!(
            honored,
            "receiver drop must unblock a UI thread already waiting on the barrier"
        );
        ui.join().unwrap();
    }

    #[test]
    fn a_scene_sent_after_receiver_death_is_dropped_not_queued() {
        // The scene half of the same contract: sending after the receiver is gone
        // must not stash a frame in a slot no one will take.
        let (tx, rx) = render_channel::<u32, ()>();
        drop(rx);
        tx.send_scene(frame(1)); // must be a no-op, not a panic or a leak
    }

    // -----------------------------------------------------------------
    // Bounded barrier waits (wait_timeout)
    // -----------------------------------------------------------------

    #[test]
    fn wait_timeout_returns_true_when_acknowledged() {
        let (waiter, ack) = ack_pair();
        ack.acknowledge();
        assert!(
            waiter.wait_timeout(REGRESSION_DEADLINE),
            "an acknowledged barrier must report honored"
        );
    }

    #[test]
    fn wait_timeout_expires_false_when_never_acknowledged() {
        // Hold the `Ack` for the whole call so it can never fire: the wait must
        // expire and report the timeout (the degrade-not-hang signal).
        let (waiter, _ack) = ack_pair();
        assert!(
            !waiter.wait_timeout(Duration::from_millis(20)),
            "wait_timeout must return false when the ack never fires"
        );
    }
}
