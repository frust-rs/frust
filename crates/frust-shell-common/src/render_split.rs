//! Render-thread-split plumbing shared by every shell (plan phase 11.B.1).
//!
//! # What lives here
//!
//! The split moves `encode→acquire→blit→present` off the UI thread onto a
//! dedicated render thread (plan phase 11.B): the UI thread keeps
//! `rebuild→layout→paint`, then hands the finished [`Scene`] across. This
//! module is the *vocabulary* for that handoff — the shells (tasks 08/09/10)
//! own the threads and the `wgpu`/`vello` resources, this crate owns the
//! platform-free channel types and the pure lifecycle/kill-switch logic they
//! coordinate through.
//!
//! - [`render_channel`] — the single UI→render link: a **depth-1, latest-wins**
//!   scene-handoff slot (a newer [`SceneFrame`] replaces an un-taken one; the
//!   render thread always takes the freshest, dropping stale frames) fused with
//!   a FIFO lifecycle-command queue behind **one** [`std::sync::Condvar`], so
//!   the render thread has a single wait point ([`RenderReceiver::wait_next`]).
//!   Depth 1 is deliberate — Flutter's merged-mode precedent (RESEARCH.md Q9:
//!   pipeline depth drops to 1 when threads merge); deeper queues add latency
//!   for no mobile win.
//! - [`RenderCommand`] / [`RenderEvent`] / [`RenderPhase`] — the surface
//!   lifecycle vocabulary (created/changed/destroyed/pause/resume) as **owned
//!   commands**, modelled on `frust-render`'s `SurfacePhase` machine: a pure,
//!   host-testable [`next_render_phase`] transition table gates whether the
//!   render thread [`may render`](RenderPhase::can_render).
//! - [`Ack`] / [`AckWaiter`] — the cross-thread acknowledgment barrier that
//!   makes [`RenderCommand::Pause`] and [`RenderCommand::SurfaceDestroyed`]
//!   *synchronous*: the UI thread blocks until the render thread has honored
//!   the command. This is the correctness anchor for the two platform hazards
//!   the plan flags — Android can destroy the `ANativeWindow` while the render
//!   thread still holds the surface, and iOS can kill a process that submits
//!   Metal work after the app backgrounds. Both are barriers, not shared
//!   mutable flags.
//! - [`SceneFrame`] / [`FrameMeta`] / [`SurfaceSize`] — the per-frame payload
//!   crossing the handoff: the scene plus the frame clock, the surface
//!   dimensions, and (for the single-emitter perf recording, plan phase 11.B.3)
//!   the UI thread's [`UiSpans`] half of the frame timing, which the render
//!   thread folds together with its own [`RenderSpans`] via
//!   [`FramePasses::from_split`](crate::perf::FramePasses::from_split).
//! - [`render_thread_enabled`] / [`NO_RENDER_THREAD_VAR`] — the single kill
//!   switch the shells consult, parsed exactly like [`crate::frame_gate`]'s
//!   `FRUST_NO_FRAME_GATE` (compile-time define *or* runtime env, any non-`"0"`
//!   value). When engaged, a shell keeps the pre-split single-thread path (kept
//!   until 11.E validates the split).
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
//! # Wiring is a later task
//!
//! This module ships the channel types + pure logic only; no shell spawns a
//! render thread yet (tasks 08/09/10 wire the desktop/Android/iOS shells).
//!
//! [`Scene`]: https://docs.rs/frust-scene
//! [`UiSpans`]: crate::perf::UiSpans
//! [`RenderSpans`]: crate::perf::RenderSpans

use std::sync::{Arc, Condvar, Mutex};

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
/// every shell consults (plan phase 11.B.3). `true` unless the
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
/// itself (plan phase 11.B.1's "frame metadata").
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
/// [`render_channel`] (plan phase 11.B): the finished scene, its
/// [`FrameMeta`], and the UI thread's [`UiSpans`] half of the frame timing
/// (the render thread is the single perf emitter — plan phase 11.B.3).
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

/// The render thread's view of surface lifecycle state (plan phase 11.B),
/// modelled on `frust-render`'s `SurfacePhase`: the render loop renders a
/// handed-off [`SceneFrame`] only while [`can_render`](Self::can_render) — i.e.
/// only in [`RenderPhase::Active`]. [`RenderPhase::Paused`] is the cross-thread
/// backgrounding barrier (a leftover scene must NOT be submitted after a
/// `Pause`, per the iOS process-kill hazard the plan flags).
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

/// Pure render-phase transition table (plan phase 11.B), the analogue of
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
/// [`render_channel`] (plan phase 11.B.1), as an **owned** value — not a shared
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
}

#[derive(Debug)]
struct Channel<S, W> {
    inbox: Mutex<Inbox<S, W>>,
    signal: Condvar,
}

/// The UI-thread handle to the render channel (plan phase 11.B.1): sends scenes
/// (latest-wins) and lifecycle commands (FIFO). Single-producer by design (the
/// UI thread), so it is deliberately not [`Clone`].
#[derive(Debug)]
pub struct RenderSender<S, W> {
    channel: Arc<Channel<S, W>>,
}

/// The render-thread handle to the render channel (plan phase 11.B.1): the
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

/// Create the UI→render channel (plan phase 11.B.1): a depth-1 latest-wins
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
    /// an un-taken scene is still in the slot it is replaced (and the drop
    /// counter incremented), so the render thread always takes the freshest.
    /// Wakes the render thread's [`RenderReceiver::wait_next`].
    pub fn send_scene(&self, frame: SceneFrame<S>) {
        let mut inbox = self.channel.inbox.lock().unwrap();
        if inbox.latest.is_some() {
            inbox.dropped += 1;
        }
        inbox.latest = Some(frame);
        drop(inbox);
        self.channel.signal.notify_one();
    }

    /// Queue a lifecycle command (FIFO) and wake the render thread. For the
    /// ack-carrying [`RenderCommand::Pause`]/[`RenderCommand::SurfaceDestroyed`]
    /// prefer [`Self::pause`]/[`Self::destroy_surface`], which build the barrier
    /// pair and return the [`AckWaiter`] to block on.
    pub fn send_command(&self, command: RenderCommand<W>) {
        let mut inbox = self.channel.inbox.lock().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

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
}
