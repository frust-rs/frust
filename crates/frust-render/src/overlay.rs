//! Overlay surfaces — a second, translucent wgpu surface positioned above a
//! hosted native view, drawing a subrange of the frame's commands (render-model
//! Phase 4, decision D10). The **segmented frame** ([`OverlayCut`],
//! [`SurfaceRenderer::encode_segmented`], [`SurfaceRenderer::submit_segmented`])
//! is the general path: one scene cut into the main surface's segment and one
//! range per overlay, recorded into one encoder and submitted once. The
//! **spike** ([`OVERLAY_SPIKE_ENV_VAR`], `FRUST_OVERLAY_SPIKE=1`) is its
//! degenerate single fixed cut, kept as it was. A renderer that is never
//! handed a cut (and never has the spike's split armed) draws exactly what it
//! drew before this module existed.
//!
//! # What an overlay is
//!
//! A frame normally lands on one swapchain. A native view a host places
//! *above* that swapchain (Mode A: macOS always, Android's default opaque
//! surface) covers whatever frust paints under it — a tooltip, a popover or a
//! drag shadow that should paint over the native control cannot. An overlay is
//! a second swapchain the host stacks above the native view, cleared to
//! transparent every frame, onto which the commands that paint *after* the
//! native view (in display-list order) are drawn instead of onto the main
//! surface. The main surface draws the commands before the cut; the overlay
//! draws the commands from the cut on.
//!
//! # The contract a host attaches to
//!
//! A host (an Android `SurfaceView`, an `NSView`/`UIView` hosting a
//! `CAMetalLayer`) supplies the platform object; this module does the rest.
//!
//! 1. **Create the platform object above the hosted view**, transparent and
//!    input-transparent: it must never take a touch, a click or focus (the
//!    native view under it and the main surface keep today's input rules).
//!    On Apple, a `CAMetalLayer` with `opaque = NO` in a view that returns
//!    `nil` from `hitTest:` and sits above the hosted `NSView`/`UIView`; on
//!    Android, a translucent `SurfaceView` that is neither focusable nor
//!    clickable.
//! 2. **On the render thread**, with the [`RenderContext`] the main surface
//!    was created from (its device must already exist — create the overlay
//!    after the main surface), call [`OverlaySurface::new`] with an
//!    [`OverlayTarget`] naming that object and its size in **physical
//!    pixels**:
//!    - Apple: `OverlayTarget::CoreAnimationLayer` with the `CAMetalLayer*`
//!      (`cfg(target_vendor = "apple")`).
//!    - Android and any raw-window-handle host:
//!      [`OverlayTarget::RawHandle`] with the display and window handles
//!      (Android: [`OverlayTarget::android_ndk_window`] over the
//!      `ANativeWindow*` the `SurfaceView`'s `Surface` yields).
//!
//!    Pass the main [`SurfaceRenderer`] too when there is one: the overlay
//!    then builds its pipelines through the same persisted driver cache.
//! 3. **Every frame** — the segmented frame: hand the main surface the
//!    frame's scene with the overlays' ranges as `cuts`
//!    ([`SurfaceRenderer::encode_segmented`]; the ranges are the differ's
//!    `ViewCommand::Overlay.command_range`s, ascending and disjoint), acquire
//!    ([`SurfaceRenderer::acquire`]), then submit with one [`OverlayCut`] per
//!    range, in the same order ([`SurfaceRenderer::submit_segmented`]): the
//!    main segment and every overlay's range are recorded into one encoder and
//!    submitted once, then presented main first, overlays after, in cut order.
//!    Where every present must land in one platform transaction (iOS
//!    `presentsWithTransaction`, a macOS `CATransaction` carrying the native
//!    views' geometry), [`SurfaceRenderer::submit_segmented_deferred`] hands
//!    them back as [`SegmentedPresents`] for
//!    [`SegmentedPresents::present_all`] inside it.
//!
//!    The spike's single cut is the older shape and still works: arm the main
//!    surface with [`SurfaceRenderer::set_overlay_split`] so it stops drawing
//!    the overlay's commands, call [`OverlaySurface::render`] with
//!    `spike_cut(scene)..scene.commands().len()` (see [`spike_cut`]) — its own
//!    encoder and its own submit — and present it with
//!    [`OverlaySurface::present`] right after the main surface's present (or
//!    [`OverlaySurface::take_deferred_present`] beside the main frame's
//!    [`DeferredPresent`]).
//! 4. **Resize** with [`OverlaySurface::resize`] (physical pixels) whenever
//!    the platform object's size changes.
//! 5. **Tear down by dropping the `OverlaySurface` before the platform object
//!    it was built on** — the same ordering the main surface keeps (drop the
//!    surface, then release the window/layer); an un-presented frame is
//!    dropped with it, never presented against a dead window.
//!
//! An overlay is full-size — the same extent and origin as the main surface,
//! so a range's device coordinates need no offset (the differ's v1 overlay
//! rect is the whole window).
//!
//! # Segments
//!
//! The main surface draws the complement of the cuts
//! ([`main_segment_ranges`], projected by [`project_main_segment`]); each
//! overlay draws its cut. Both go through
//! [`frust_engine::renderer::project_command_ranges`]'s bracket rule, so every
//! segment is a well-nested display list: a clip a cut falls inside clips both
//! sides, and every drawing command of the frame lands in exactly one segment.
//!
//! Only a **servable** cut is taken ([`cut_is_servable`]): one whose range
//! the overlay's engine draws exactly as the main surface would. A range
//! holding a `DrawRetained`, a `PushRetained` bracket, a `SceneTexture` or a
//! `BackdropBlur`, or one a retained bracket is open across (opened before
//! the range and still open at its start, or opened inside it and still open
//! past its end), is **kept on the main surface**: the main segment draws it
//! exactly as an uncut frame would, under the native view, and that overlay is
//! handed an empty range ([`CutServiceability::served_range`]) and clears to
//! transparent — the same degrade a host whose overlay was refused already
//! has. [`OverlayCutOutcome::serviceability`] says which cuts were kept and
//! why. Without the rule such content would draw on no surface at all (the
//! main segment excludes it, the overlay cannot draw it), or a retained page
//! would be repainted on the overlay while the main store kept advertising
//! the old one.
//! A differ cut starts AT its slot's index, so a Mode B slot's own hole-punch
//! (`ClearRect`) is the overlay's first command — drawn over the overlay's
//! transparent clear, where it erases nothing.
//!
//! Each target compiles its own segment: the engine's compiled frame is
//! specific to its target's size and root, so a segmented frame is one compile
//! per target — the main segment's plus one per overlay — never one shared
//! compile. Every engine records and frust-render submits once per frame (the
//! engine's own atlas-growth and glyph-replay submits aside): the overlays'
//! passes ride the main frame's command buffer.
//!
//! # Alpha
//!
//! The overlay asks for a translucent composite mode
//! ([`frust_gpu::surface::resolve_alpha_mode`]'s preference order, `Inherit`
//! then `PostMultiplied` then `PreMultiplied`) and **refuses** with
//! [`OverlayError::NoTranslucentMode`] when the platform grants none: an
//! opaque overlay would black out everything under it. It draws the engine's
//! premultiplied output straight into the swapchain, which is right for every
//! mode the platform composites premultiplied — Android's `Inherit` and
//! `PreMultiplied`, and Metal's `PostMultiplied` (Metal reads that mode
//! premultiplied despite its name; see `crate::context::choose_engine_render_path`).
//! A compositor that genuinely reads straight alpha is refused with
//! [`OverlayError::StraightAlpha`] rather than served half-right.
//!
//! # What the overlay renderer is
//!
//! Its own [`frust_engine::EngineRenderer`] on the main surface's device and
//! queue ([`OverlayRenderer`]), warmed for the overlay's format and built with
//! **retention off**: a retained page belongs to the main surface's store, so
//! a `DrawRetained` handed to the overlay is dropped (counted in
//! [`RangeCut::dropped_draw_retained`]) and a `PushRetained` body is painted
//! inline. The range is cut by [`frust_engine::renderer::project_command_range`]
//! — read its *bracket rule* for what happens to a clip, layer or retained
//! bracket a cut falls inside. A backdrop blur handed to the overlay draws
//! nothing (its swapchain cannot be read back), and an external-pass texture
//! (`SceneTexture`) is bound in the main renderer's registry only, so it draws
//! nothing on the overlay either — which is why the segmented frame never
//! hands an overlay any of them (the servability rule above) and a served
//! cut's [`RangeCut::dropped_draw_retained`] is always zero. The spike's
//! single fixed cut ([`OverlaySurface::render`]) applies no such rule.
//! Shader quads are prepared over the whole scene by every overlay (the
//! pre-pass is per scene, on the overlay's own engine), so the overlay serves
//! a quad in its range, and one outside it is rendered and never sampled
//! there. Each overlay engine schedules its own budget.
//!
//! # The sanctioned `unsafe` zone
//!
//! [`OverlaySurface::new`] is the one `unsafe` function here: it turns a
//! caller-owned raw window handle or `CAMetalLayer*` into a `wgpu::Surface`
//! (`wgpu::Instance::create_surface_unsafe`), the same crossing
//! `frust_gpu::lifecycle::create_android_surface`/`create_metal_surface` make
//! for the main surface. Its `# Safety` section is the whole contract.

use core::ffi::c_void;
use core::ops::Range;
use core::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use frust_engine::renderer::{RangeCut, open_retained_bracket};
use frust_gpu::{RenderContext, SurfaceAlphaRequest};
use frust_scene::Command;
use kurbo::Affine;

use crate::context::{EngineSurface, RenderPath};
use crate::renderer::{DeferredPresent, GpuPassTimings, SurfaceRenderer};

/// The spike's switch: `FRUST_OVERLAY_SPIKE`, read at compile time
/// (`option_env!`, so a `frust build --define` bakes it into an Android or iOS
/// app with no runtime environment) or at runtime, non-zero meaning on.
///
/// Off — the default — nothing in this module is reached by any shipped
/// path: [`SurfaceRenderer::set_overlay_split`] refuses to arm and a shell
/// creates no overlay.
pub const OVERLAY_SPIKE_ENV_VAR: &str = "FRUST_OVERLAY_SPIKE";

/// Whether the overlay spike is switched on ([`OVERLAY_SPIKE_ENV_VAR`]).
///
/// Read once per process and cached, like every `FRUST_*` knob; a runtime
/// value wins over neither — either half set non-zero switches it on.
pub fn overlay_spike_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| spike_value().is_some())
}

/// The spike switch's raw value when it is on, else `None` — a shell that
/// offers more than one host arrangement under the spike (Android's z-order
/// A/B, see `frust-shell-android`) reads its variant from here.
pub fn overlay_spike_value() -> Option<String> {
    spike_value()
}

fn spike_value() -> Option<String> {
    let set = |value: &str| {
        let value = value.trim();
        (!value.is_empty() && value != "0").then(|| value.to_string())
    };
    std::env::var(OVERLAY_SPIKE_ENV_VAR)
        .ok()
        .as_deref()
        .and_then(set)
        .or_else(|| option_env!("FRUST_OVERLAY_SPIKE").and_then(set))
}

/// The spike's fixed cut: the command index just past the scene's first
/// `ClearRect`, or `None` when it records none.
///
/// A `ClearRect` is the one command a platform view leaves in the display
/// list: on a translucent (Mode B) surface it is the slot's own hole punch, so
/// everything after it paints above the first native view. An opaque (Mode A)
/// surface records no punch, so a host page that wants an overlay under the
/// spike records a zero-area `ClearRect` right after its native view as the
/// marker (the native-widgets demo's Controls page does) — a zero-area clear
/// erases nothing on either surface.
///
/// Fixed and deliberately crude: the production cut is the platform view's
/// own paint-order index (p4-02), and a frame may cut more than once (p4-03).
pub fn spike_cut(scene: &frust_scene::Scene) -> Option<usize> {
    scene
        .commands()
        .iter()
        .position(|command| matches!(command, frust_scene::Command::ClearRect { .. }))
        .map(|at| at + 1)
}

/// The platform object an [`OverlaySurface`] presents into.
///
/// The variants mirror `wgpu::SurfaceTargetUnsafe`'s raw ones; the pointer
/// and handles are the caller's, borrowed for the surface's whole life (see
/// [`OverlaySurface::new`]'s `# Safety`).
#[derive(Debug, Clone, Copy)]
pub enum OverlayTarget {
    /// A raw display and window handle — Android's `ANativeWindow`
    /// ([`Self::android_ndk_window`]) and any other raw-window-handle host.
    RawHandle {
        /// The display the window lives on.
        display: wgpu::rwh::RawDisplayHandle,
        /// The window to present into.
        window: wgpu::rwh::RawWindowHandle,
    },
    /// A `CAMetalLayer*` an `NSView`/`UIView` hosts above the native view —
    /// macOS and iOS (`p4-01-mac`).
    #[cfg(target_vendor = "apple")]
    CoreAnimationLayer(*mut c_void),
}

impl OverlayTarget {
    /// The Android target for an acquired `ANativeWindow*`.
    pub fn android_ndk_window(window: NonNull<c_void>) -> Self {
        use wgpu::rwh::{
            AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        Self::RawHandle {
            display: RawDisplayHandle::Android(AndroidDisplayHandle::new()),
            window: RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window)),
        }
    }
}

/// Why an overlay could not be created or could not draw a frame.
#[derive(Debug)]
#[non_exhaustive]
pub enum OverlayError {
    /// The context has no device yet: an overlay shares the main surface's
    /// device, so it is created after the main surface.
    NoDevice,
    /// The platform object could not be turned into a surface, or the
    /// surface could not be configured.
    Surface(String),
    /// The main surface's adapter cannot present to this surface. Refused
    /// rather than answered with a second device: the overlay must share the
    /// main frame's device.
    UnsupportedSurface,
    /// The platform granted no translucent composite mode — an opaque overlay
    /// would cover everything under it.
    NoTranslucentMode,
    /// The compositor reads the overlay's alpha straight, which the spike
    /// does not serve (no un-premultiplying pass).
    StraightAlpha,
    /// The overlay's engine renderer could not be built or refused the frame.
    Engine(frust_engine::EngineError),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevice => f.write_str("overlay: no device yet (create the main surface first)"),
            Self::Surface(reason) => write!(f, "overlay: surface creation failed: {reason}"),
            Self::UnsupportedSurface => {
                f.write_str("overlay: the main surface's adapter cannot present to this surface")
            }
            Self::NoTranslucentMode => {
                f.write_str("overlay: the platform granted no translucent composite mode")
            }
            Self::StraightAlpha => {
                f.write_str("overlay: the compositor reads straight alpha, which the spike refuses")
            }
            Self::Engine(error) => write!(f, "overlay: engine: {error}"),
        }
    }
}

impl std::error::Error for OverlayError {}

/// The overlay's renderer, free of any surface: a retention-off
/// [`frust_engine::EngineRenderer`] that records a command range into a
/// caller-supplied target cleared to transparent.
///
/// [`OverlaySurface`] wraps one around a swapchain; the spike's host tests
/// drive one directly against an offscreen target.
pub struct OverlayRenderer {
    engine: Box<frust_engine::EngineRenderer>,
    shader_quads: Box<frust_engine::ShaderQuadPass>,
    timestamps: frust_gpu::diag::TimestampRing,
    last_cut: Option<RangeCut>,
    refused: u32,
}

impl OverlayRenderer {
    /// Builds an overlay renderer for `format` targets on `device`.
    ///
    /// `pipeline_cache` is the main surface's persisted driver cache, when it
    /// has one, so the overlay's pipelines compile as cache hits.
    ///
    /// # Errors
    ///
    /// [`OverlayError::Engine`] when the engine cannot be built on `caps`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        caps: &frust_gpu::TierCaps,
        format: wgpu::TextureFormat,
        pipeline_cache: Option<&wgpu::PipelineCache>,
    ) -> Result<Self, OverlayError> {
        let mut engine = frust_engine::EngineRenderer::new(device, caps, format, pipeline_cache)
            .map_err(OverlayError::Engine)?;
        // The retained store is the main surface's: the overlay records no page
        // and draws none (see the module docs).
        engine.set_retained_layers(false);
        Ok(Self {
            engine: Box::new(engine),
            shader_quads: Box::new(frust_engine::ShaderQuadPass::new(pipeline_cache.cloned())),
            timestamps: frust_gpu::diag::TimestampRing::new(
                device,
                queue,
                frust_engine::EngineSpan::COUNT,
                "frust-render overlay timestamps",
            ),
            last_cut: None,
            refused: 0,
        })
    }

    /// Records the commands of `scene` in `range` into `encoder`, targeting
    /// `target` cleared to transparent, and returns what the cut did.
    ///
    /// Shader quads are prepared over the whole `scene` (the pre-pass is
    /// per-scene), so a quad outside the range is rendered to its own
    /// offscreen target too and simply never sampled — a spike cost.
    ///
    /// # Errors
    ///
    /// [`OverlayError::Engine`] when the engine refuses the frame; nothing
    /// was recorded into `encoder` then.
    pub fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &frust_scene::Scene,
        range: Range<usize>,
        target: frust_engine::EngineTarget<'_>,
    ) -> Result<RangeCut, OverlayError> {
        self.shader_quads.prepare(
            device,
            queue,
            encoder,
            scene,
            Affine::IDENTITY,
            &mut self.engine,
        );
        self.timestamps.begin_frame(device);
        let result = self.engine.record_range(
            device,
            queue,
            encoder,
            scene,
            range,
            target,
            peniko::Color::TRANSPARENT,
            Affine::IDENTITY,
            frust_engine::FrameTimestamps::new(&self.timestamps),
        );
        match result {
            Ok(cut) => {
                self.timestamps.resolve(encoder);
                self.last_cut = Some(cut);
                Ok(cut)
            }
            Err(error) => {
                self.timestamps.abandon_frame();
                self.refused = self.refused.saturating_add(1);
                Err(OverlayError::Engine(error))
            }
        }
    }

    /// Closes the frame [`Self::record`] recorded, once its encoder was
    /// submitted (`submitted`) or dropped: the engine's end-of-frame
    /// maintenance and the timestamp ring's frame close — or, for an encoder
    /// that was never submitted, the ring's abandon, so no slot is left
    /// waiting on a resolve that will never run.
    pub fn end_frame(&mut self, queue: &wgpu::Queue, submitted: bool) {
        if submitted {
            self.timestamps.end_frame();
        } else {
            self.timestamps.abandon_frame();
        }
        self.engine.end_frame(queue);
    }

    /// What the last recorded range's cut did, `None` before the first.
    pub fn last_cut(&self) -> Option<RangeCut> {
        self.last_cut
    }

    /// How many frames the engine has refused on this overlay.
    pub fn refused_frames(&self) -> u32 {
        self.refused
    }

    /// The overlay's most recent per-pass GPU time, on a device with
    /// `TIMESTAMP_QUERY` (a `perf-trace` build); `None` otherwise — the same
    /// reading [`SurfaceRenderer::gpu_pass_timings`] gives the main surface.
    pub fn gpu_pass_timings(&self) -> Option<GpuPassTimings> {
        let reading = self.timestamps.latest()?;
        let span = |which: frust_engine::EngineSpan| reading.span(which.index());
        Some(GpuPassTimings {
            prepass: span(frust_engine::EngineSpan::Prepass),
            main: span(frust_engine::EngineSpan::Main),
            composite: span(frust_engine::EngineSpan::Composite),
            blit: span(frust_engine::EngineSpan::Blit),
        })
    }
}

/// What one [`OverlaySurface::record`] (or [`OverlaySurface::render`]) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayFrame {
    /// The range was drawn and its frame is waiting for
    /// [`OverlaySurface::present`] (or [`OverlaySurface::take_deferred_present`])
    /// — after [`OverlaySurface::record`], once its encoder was submitted and
    /// [`OverlaySurface::end_frame`] told so.
    Rendered(RangeCut),
    /// The swapchain was stale and has been reconfigured; nothing to present
    /// this frame.
    Reconfigured,
    /// No frame this time (a transient acquire failure, or a lost surface the
    /// host recreates through its own lifecycle).
    Skipped,
}

/// A translucent swapchain above a hosted native view, drawing one command
/// range of the frame — see the [module docs](self) for the host contract.
pub struct OverlaySurface {
    surface: EngineSurface,
    renderer: OverlayRenderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The frame [`Self::record`] acquired and recorded, until
    /// [`Self::end_frame`] learns whether its encoder was submitted.
    recorded: Option<wgpu::SurfaceTexture>,
    /// The submitted frame waiting for [`Self::present`].
    pending: Option<wgpu::SurfaceTexture>,
}

impl OverlaySurface {
    /// Creates the overlay's swapchain on `target`, configured translucent at
    /// `width` x `height` physical pixels, sharing `ctx`'s device and queue.
    ///
    /// `main` is the main surface's renderer when there is one; the overlay
    /// then compiles its pipelines through the same persisted driver cache.
    ///
    /// # Safety
    ///
    /// The one `unsafe` crossing of this module (a sanctioned zone, like
    /// `frust_gpu::lifecycle::create_android_surface`): `target` must name a
    /// valid, live platform object — an acquired `ANativeWindow*` behind an
    /// Android NDK handle, a valid display/window pair, or a live
    /// `CAMetalLayer*` — that **outlives** the returned `OverlaySurface` and
    /// every frame it acquires. Drop the `OverlaySurface` before releasing
    /// the window or tearing down the layer.
    ///
    /// # Errors
    ///
    /// [`OverlayError::NoDevice`] before the main surface created the device;
    /// [`OverlayError::Surface`] when the platform object yields no surface or
    /// the swapchain cannot be configured; [`OverlayError::UnsupportedSurface`]
    /// when the shared adapter cannot present to it;
    /// [`OverlayError::NoTranslucentMode`]/[`OverlayError::StraightAlpha`] for
    /// an alpha mode the overlay cannot use; [`OverlayError::Engine`] when the
    /// overlay's renderer cannot be built.
    pub unsafe fn new(
        ctx: &mut RenderContext,
        main: Option<&SurfaceRenderer>,
        target: OverlayTarget,
        width: u32,
        height: u32,
    ) -> Result<Self, OverlayError> {
        if ctx.caps().is_none() {
            return Err(OverlayError::NoDevice);
        }
        let raw = match target {
            OverlayTarget::RawHandle { display, window } => wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(display),
                raw_window_handle: window,
            },
            #[cfg(target_vendor = "apple")]
            OverlayTarget::CoreAnimationLayer(layer) => {
                if layer.is_null() {
                    return Err(OverlayError::Surface("null CAMetalLayer pointer".into()));
                }
                wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer)
            }
        };
        // SAFETY: upheld by this function's own `# Safety` contract — the
        // platform object `target` names is valid and outlives the surface.
        let surface = unsafe { ctx.instance().create_surface_unsafe(raw) }
            .map_err(|error| OverlayError::Surface(error.to_string()))?;
        // Asked before configuring: `create_engine_surface` would otherwise
        // answer an unsupported surface with a NEW device, pulling the device
        // out from under the main surface.
        if !ctx.device_handle().adapter.is_surface_supported(&surface) {
            return Err(OverlayError::UnsupportedSurface);
        }
        let surface = pollster_block_on(crate::context::create_engine_surface(
            ctx,
            surface,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
            SurfaceAlphaRequest::TranslucentPreferred,
        ))
        .map_err(|error| OverlayError::Surface(format!("{error:#}")))?;
        if !surface.resolved_translucent() {
            return Err(OverlayError::NoTranslucentMode);
        }
        if !matches!(surface.path, RenderPath::EngineDirect { .. }) {
            return Err(OverlayError::StraightAlpha);
        }
        let handle = ctx.device_handle();
        let cache = main.and_then(SurfaceRenderer::shared_pipeline_cache);
        let renderer = OverlayRenderer::new(
            &handle.device,
            &handle.queue,
            &handle.caps,
            surface.config().format,
            cache.as_ref(),
        )?;
        log::info!(
            "frust-render overlay spike: overlay surface up ({:?}, {:?}, {}x{})",
            surface.config().format,
            surface.config().alpha_mode,
            surface.config().width,
            surface.config().height
        );
        Ok(Self {
            surface,
            renderer,
            device: handle.device.clone(),
            queue: handle.queue.clone(),
            recorded: None,
            pending: None,
        })
    }

    /// The swapchain's `(width, height)` in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.surface.config().width, self.surface.config().height)
    }

    /// Resizes the swapchain (physical pixels); a zero extent is ignored, and
    /// an un-presented frame of the old size is dropped.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == self.size() {
            return;
        }
        self.pending = None;
        if self.recorded.take().is_some() {
            self.renderer.end_frame(&self.queue, false);
        }
        self.surface.resize(&self.device, width, height);
    }

    /// Acquires the overlay's next frame and records the commands of `scene`
    /// in `range` into it over a transparent clear — into the **caller's**
    /// `encoder`, so the overlay's passes land in the same command buffer as
    /// the main frame's and one queue submit carries both (the segmented
    /// frame, [`SurfaceRenderer::submit_segmented`]).
    ///
    /// Nothing is submitted and nothing is presentable yet: the acquired
    /// frame waits, recorded, until [`Self::end_frame`] is told whether the
    /// encoder was submitted — then (and only then) it becomes the frame
    /// [`Self::present`]/[`Self::take_deferred_present`] hand out. A frame
    /// still waiting from an earlier call (presentable or merely recorded) is
    /// dropped un-presented first (latest wins).
    ///
    /// [`OverlayFrame::Reconfigured`]/[`OverlayFrame::Skipped`] recorded
    /// nothing and leave nothing for [`Self::end_frame`] to close.
    ///
    /// # Errors
    ///
    /// [`OverlayError::Engine`] when the engine refuses the frame: the
    /// acquired frame is dropped and the renderer's frame already closed, so
    /// there is nothing for [`Self::end_frame`] to do (the previous overlay
    /// frame stays on screen). Shader-quad pre-passes the overlay recorded
    /// ahead of the refusal stay in `encoder`, harmlessly — their targets are
    /// the overlay's own.
    pub fn record(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &frust_scene::Scene,
        range: Range<usize>,
    ) -> Result<OverlayFrame, OverlayError> {
        self.pending = None;
        if self.recorded.take().is_some() {
            // Recorded by an earlier call whose encoder never reported back:
            // close that frame as unsubmitted before opening this one.
            self.renderer.end_frame(&self.queue, false);
        }
        use wgpu::CurrentSurfaceTexture as Cst;
        let texture = match self.surface.gpu.surface().get_current_texture() {
            Cst::Success(texture) | Cst::Suboptimal(texture) => texture,
            Cst::Outdated | Cst::Validation => {
                self.surface.reconfigure(&self.device);
                return Ok(OverlayFrame::Reconfigured);
            }
            Cst::Lost | Cst::Timeout | Cst::Occluded => return Ok(OverlayFrame::Skipped),
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (width, height) = self.size();
        let format = self.surface.config().format;
        let depth = match &self.surface.path {
            RenderPath::EngineDirect { depth, .. } => Some(depth.view()),
            RenderPath::EngineDirectUnpremultiply { .. } => None,
        };
        let result = self.renderer.record(
            &self.device,
            &self.queue,
            encoder,
            scene,
            range,
            frust_engine::EngineTarget {
                view: &view,
                format,
                width,
                height,
                depth,
                output: frust_engine::OutputAlpha::Premultiplied,
            },
        );
        match result {
            Ok(cut) => {
                self.recorded = Some(texture);
                Ok(OverlayFrame::Rendered(cut))
            }
            Err(error) => {
                self.renderer.end_frame(&self.queue, false);
                Err(error)
            }
        }
    }

    /// Closes the frame [`Self::record`] recorded, once the encoder it was
    /// recorded into has been submitted (`submitted`) or dropped: the
    /// renderer's end-of-frame maintenance, and — when submitted — the
    /// recorded frame becomes the one [`Self::present`] presents. Answers
    /// whether a frame is now waiting to be presented; a no-op answering
    /// `false` when nothing was recorded.
    pub fn end_frame(&mut self, submitted: bool) -> bool {
        let Some(texture) = self.recorded.take() else {
            return false;
        };
        self.renderer.end_frame(&self.queue, submitted);
        if submitted {
            self.pending = Some(texture);
        }
        submitted
    }

    /// **The spike's path only** — [`Self::record`] into an encoder of the
    /// overlay's own, submitted on its own: a second queue submit per frame,
    /// which the segmented frame exists to remove
    /// ([`SurfaceRenderer::submit_segmented`] records every overlay into the
    /// main frame's encoder and submits once). Kept for the single-cut spike
    /// host (`FRUST_OVERLAY_SPIKE`), which renders its overlay ahead of the
    /// main frame.
    ///
    /// Leaves the frame un-presented until [`Self::present`] (or
    /// [`Self::take_deferred_present`]), so the host can land it right beside
    /// the main frame's present. A frame still waiting from the last call is
    /// dropped un-presented first (latest wins).
    ///
    /// # Errors
    ///
    /// [`OverlayError::Engine`] when the engine refuses the frame (nothing is
    /// presented; the previous overlay frame stays on screen).
    pub fn render(
        &mut self,
        scene: &frust_scene::Scene,
        range: Range<usize>,
    ) -> Result<OverlayFrame, OverlayError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frust-render overlay"),
            });
        let frame = self.record(&mut encoder, scene, range)?;
        if matches!(frame, OverlayFrame::Rendered(_)) {
            self.queue.submit([encoder.finish()]);
            self.end_frame(true);
        }
        Ok(frame)
    }

    /// Presents the frame [`Self::render`] (or a submitted [`Self::record`])
    /// left waiting, on the calling thread; `false` when there was none.
    pub fn present(&mut self) -> bool {
        match self.pending.take() {
            Some(texture) => {
                self.queue.present(texture);
                true
            }
            None => false,
        }
    }

    /// Hands the waiting frame out as a [`DeferredPresent`], for a host that
    /// presents the main frame and the overlay inside one platform
    /// transaction (iOS `presentsWithTransaction`); `None` when there was
    /// none.
    pub fn take_deferred_present(&mut self) -> Option<DeferredPresent> {
        self.pending
            .take()
            .map(|texture| DeferredPresent::new(texture, self.queue.clone()))
    }

    /// The overlay's renderer — its cut report, refusal count and GPU time.
    pub fn renderer(&self) -> &OverlayRenderer {
        &self.renderer
    }
}

impl std::fmt::Debug for OverlaySurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlaySurface")
            .field("size", &self.size())
            .field("pending", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}

/// One overlay of a segmented frame: the [`OverlaySurface`] that draws the
/// command range `range` of the frame's scene, named by the differ's
/// `overlay_id` (`frust_shell_common`'s `ViewCommand::Overlay`, whose
/// `command_range` is this `range`).
///
/// Handed to [`SurfaceRenderer::submit_segmented`] (or
/// [`SurfaceRenderer::submit_segmented_deferred`]) with the same ranges
/// [`SurfaceRenderer::encode_segmented`] was given as `cuts`, in the same
/// order.
#[derive(Debug)]
pub struct OverlayCut<'a> {
    /// The overlay drawing this range.
    pub overlay: &'a mut OverlaySurface,
    /// The half-open command range of the frame's scene it draws.
    pub range: Range<usize>,
    /// The differ's id for the overlay, carried back in
    /// [`OverlayCutOutcome::overlay_id`] and [`SegmentedPresents::overlays`].
    pub overlay_id: u64,
}

/// What one [`OverlayCut`] of a segmented frame did.
#[derive(Debug)]
pub struct OverlayCutOutcome {
    /// The cut's [`OverlayCut::overlay_id`].
    pub overlay_id: u64,
    /// [`OverlaySurface::record`]'s answer for this frame —
    /// [`OverlayFrame::Skipped`] for every cut of a frame whose main segment
    /// was not rendered (nothing was recorded for any overlay then).
    pub result: Result<OverlayFrame, OverlayError>,
    /// Whether the cut's range was served on the overlay or kept on the main
    /// surface, and why ([`cut_is_servable`]): a kept cut's overlay recorded
    /// an empty range, so its `result` is a transparent frame. The
    /// classification [`SurfaceRenderer::encode_segmented`] made for the
    /// frame — [`CutServiceability::Servable`] for a cut the frame's encode
    /// never classified (a plain [`SurfaceRenderer::encode`]).
    pub serviceability: CutServiceability,
}

/// Why a cut's range cannot be drawn on an overlay and stays on the main
/// surface — [`CutServiceability::Unservable`]'s reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnservableReason {
    /// A `DrawRetained`: the page lives in the main surface's retained store,
    /// which the overlay's engine cannot read.
    RetainedComposite,
    /// A `PushRetained` bracket inside the range: painted inline on the
    /// overlay, it would never record the page the main surface's store then
    /// keeps advertising, and the same content would move between surfaces
    /// as the widget turns clean (drawn from its page) and dirty again.
    RetainedRecording,
    /// A retained bracket open across one of the range's ends: the projection
    /// would demote it to inline paint on both sides of the cut.
    RetainedBracketCrossing,
    /// A `SceneTexture`: external textures are bound in the main renderer's
    /// registry only.
    SceneTexture,
    /// A `BackdropBlur`: it reads back what is painted beneath it, which an
    /// overlay's swapchain does not hold.
    BackdropBlur,
}

impl std::fmt::Display for UnservableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RetainedComposite => "retained content (a page composite)",
            Self::RetainedRecording => "retained content (a page recording)",
            Self::RetainedBracketCrossing => "retained content (a bracket crossing the cut)",
            Self::SceneTexture => "an external texture",
            Self::BackdropBlur => "a backdrop blur",
        })
    }
}

/// Whether an overlay can draw a cut's range exactly as the main surface
/// would — [`cut_is_servable`]'s answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CutServiceability {
    /// The overlay draws the range; the main segment excludes it.
    #[default]
    Servable,
    /// The range stays on the main surface and the overlay draws nothing.
    Unservable {
        /// Why.
        reason: UnservableReason,
        /// The command index that decided it: the offending command, or the
        /// `PushRetained` opening the crossing bracket.
        at: usize,
    },
}

impl CutServiceability {
    /// Whether the overlay serves the cut.
    pub fn is_servable(self) -> bool {
        matches!(self, Self::Servable)
    }

    /// The range the overlay records for `cut`, and the main segment
    /// excludes: `cut` itself when servable, otherwise the empty range `0..0`
    /// — nothing reopened, nothing drawn, the overlay cleared to transparent.
    pub fn served_range(self, cut: &Range<usize>) -> Range<usize> {
        match self {
            Self::Servable => cut.clone(),
            Self::Unservable { .. } => 0..0,
        }
    }
}

impl std::fmt::Display for CutServiceability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Servable => f.write_str("served on the overlay"),
            Self::Unservable { reason, at } => {
                write!(f, "kept on main: {reason} at command {at}")
            }
        }
    }
}

/// Classifies the cut `range` of `commands`: whether an overlay can draw it
/// exactly as the main surface would (see the module docs' *Segments*).
///
/// Unservable, at the first offending command in display-list order: a
/// retained bracket open at the range's start
/// ([`frust_engine::renderer::open_retained_bracket`]); then, inside the
/// range, a `DrawRetained`, a `PushRetained` (whose bracket crosses the
/// range's end when it is still open there short of the scene's end), a
/// `SceneTexture` or a `BackdropBlur`. Everything else — fills, paths, text,
/// images, blurred rects, hole punches, clips, layers and shader quads (the
/// overlay prepares its own) — is servable. `range` is clamped to the list
/// the way the projection clamps it, and an empty range is servable (there is
/// nothing to draw on either surface). Pure and read-only: one pass over the
/// commands up to the range's end.
pub fn cut_is_servable(commands: &[Command], range: Range<usize>) -> CutServiceability {
    let len = commands.len();
    let end = range.end.min(len);
    let start = range.start.min(end);
    if start == end {
        return CutServiceability::Servable;
    }
    let unservable = |reason, at| CutServiceability::Unservable { reason, at };
    if let Some(at) = open_retained_bracket(commands, start) {
        return unservable(UnservableReason::RetainedBracketCrossing, at);
    }
    for (at, command) in commands.iter().enumerate().take(end).skip(start) {
        let reason = match command {
            Command::DrawRetained { .. } => UnservableReason::RetainedComposite,
            Command::PushRetained { .. } => {
                if end < len && !retained_closes_before(commands, at, end) {
                    UnservableReason::RetainedBracketCrossing
                } else {
                    UnservableReason::RetainedRecording
                }
            }
            Command::SceneTexture { .. } => UnservableReason::SceneTexture,
            Command::BackdropBlur { .. } => UnservableReason::BackdropBlur,
            _ => continue,
        };
        return unservable(reason, at);
    }
    CutServiceability::Servable
}

/// Whether the retained bracket `commands[push]` opens is closed before
/// `end` — pops matched to the most recent retained push, as the projection
/// matches them.
fn retained_closes_before(commands: &[Command], push: usize, end: usize) -> bool {
    let mut depth = 0_usize;
    for command in commands.iter().take(end).skip(push) {
        match command {
            Command::PushRetained { .. } => depth += 1,
            Command::PopRetained => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// What [`SurfaceRenderer::submit_segmented`] did with one frame.
#[derive(Debug)]
pub struct SegmentedOutcome {
    /// The main surface's outcome — exactly what [`SurfaceRenderer::submit`]
    /// would have answered.
    pub frame: frust_gpu::FrameOutcome,
    /// One entry per [`OverlayCut`], in the order given.
    pub overlays: Vec<OverlayCutOutcome>,
    /// Command encoders the frame created: `1` whenever it reached the GPU
    /// encode — the main segment and every overlay share it (an engine
    /// refusal drops it un-submitted) — `0` when it never got that far.
    pub encoders: u32,
    /// Queue submits the frame issued: `1` for a rendered frame, however many
    /// overlays it carried, `0` otherwise. The segmented frame's invariant.
    pub submits: u32,
}

/// The un-presented frames of one segmented frame —
/// [`SurfaceRenderer::submit_segmented_deferred`]'s answer for a host that
/// pairs the presents itself (iOS/macOS inside one `CATransaction`).
///
/// Dropping it presents nothing: every frame returns to its swapchain
/// un-presented, the same skip a dropped [`DeferredPresent`] is.
#[derive(Debug, Default)]
pub struct SegmentedPresents {
    /// The main surface's frame, `None` when the main frame did not render.
    pub main: Option<DeferredPresent>,
    /// Each overlay that rendered, with its [`OverlayCut::overlay_id`], in cut
    /// order.
    pub overlays: Vec<(u64, DeferredPresent)>,
}

impl SegmentedPresents {
    /// Presents every frame on the calling thread: the main surface first,
    /// then each overlay in cut order (bottom to top).
    pub fn present_all(self) {
        if let Some(main) = self.main {
            main.present();
        }
        for (_, overlay) in self.overlays {
            overlay.present();
        }
    }

    /// The frames as one list — the main surface's first, then each overlay
    /// in cut order — for a host that presents them through a list of its own.
    pub fn into_vec(self) -> Vec<DeferredPresent> {
        self.main
            .into_iter()
            .chain(self.overlays.into_iter().map(|(_, present)| present))
            .collect()
    }

    /// How many frames are waiting.
    pub fn len(&self) -> usize {
        usize::from(self.main.is_some()) + self.overlays.len()
    }

    /// Whether no frame is waiting.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The ranges of a `len`-command scene the **main** surface draws when the
/// overlays draw `cuts`: the complement of the cuts, ascending, empty gaps
/// left out — `0..first.start`, the gap between each cut and the next, and
/// whatever follows the last cut.
///
/// `cuts` are the differ's overlay ranges: ascending and disjoint
/// (`ViewCommand::Overlay.command_range`). Each is clamped to the scene; a cut
/// that starts before the previous one ended is treated as starting where it
/// ended, so a malformed list never makes the main surface draw a command
/// twice or go backwards. An empty cut (an overlay whose content disappeared,
/// `command_range` empty) removes nothing and splits no gap.
pub fn main_segment_ranges(len: usize, cuts: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(cuts.len() + 1);
    let mut from = 0;
    for cut in cuts {
        let start = cut.start.clamp(from, len);
        let end = cut.end.clamp(start, len);
        if start == end {
            // An empty cut (an overlay whose content disappeared) removes
            // nothing, so it splits nothing either.
            continue;
        }
        if from < start {
            ranges.push(from..start);
        }
        from = end;
    }
    if from < len {
        ranges.push(from..len);
    }
    ranges
}

/// Projects the main surface's segment of `scene` — every command outside the
/// overlay `cuts` ([`main_segment_ranges`]) — into `out`, through
/// [`frust_engine::renderer::project_command_ranges`]: each gap between cuts
/// is projected on its own, so its brackets are rebalanced (a clip open across
/// a cut closes before it and reopens after it) and the concatenation is a
/// well-nested display list. What [`SurfaceRenderer::encode_segmented`] keeps
/// for the main surface.
pub fn project_main_segment(
    scene: &frust_scene::Scene,
    cuts: &[Range<usize>],
    options: frust_engine::renderer::RangeOptions,
    out: &mut frust_scene::Scene,
) -> RangeCut {
    let ranges = main_segment_ranges(scene.commands().len(), cuts);
    frust_engine::renderer::project_command_ranges(scene, &ranges, options, out)
}

/// Records every cut's served range of `scene` into `encoder` — the overlay
/// half of [`SurfaceRenderer::submit_segmented`], after the main segment's
/// passes and before the one submit.
///
/// `service` is the frame's classification, one per cut in order, as the
/// encode cut the main segment by; a cut past its end is classified here.
/// A kept cut's overlay records the empty range
/// ([`CutServiceability::served_range`]).
pub(crate) fn record_cuts(
    encoder: &mut wgpu::CommandEncoder,
    scene: &frust_scene::Scene,
    cuts: &mut [OverlayCut<'_>],
    service: &[CutServiceability],
) -> Vec<OverlayCutOutcome> {
    cuts.iter_mut()
        .enumerate()
        .map(|(index, cut)| {
            let serviceability = service
                .get(index)
                .copied()
                .unwrap_or_else(|| cut_is_servable(scene.commands(), cut.range.clone()));
            let result =
                cut.overlay
                    .record(encoder, scene, serviceability.served_range(&cut.range));
            if let Ok(OverlayFrame::Rendered(range_cut)) = &result {
                note_dropped_composites(range_cut);
            }
            OverlayCutOutcome {
                overlay_id: cut.overlay_id,
                result,
                serviceability,
            }
        })
        .collect()
}

/// A served cut never hands the overlay a `DrawRetained` ([`cut_is_servable`]
/// keeps such a range on the main surface), so the overlay's projection drops
/// none: asserted in a debug build, logged once otherwise — a dropped one is
/// content drawn on no surface.
fn note_dropped_composites(cut: &RangeCut) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    debug_assert_eq!(
        cut.dropped_draw_retained, 0,
        "an overlay dropped a retained composite of a served cut"
    );
    if cut.dropped_draw_retained > 0 && !LOGGED.swap(true, Ordering::Relaxed) {
        log::error!(
            "frust-render overlay: a served cut dropped {} retained composite(s) \
             (commands {}..{}); that content drew on no surface (logged once)",
            cut.dropped_draw_retained,
            cut.start,
            cut.end
        );
    }
}

/// One [`OverlayFrame::Skipped`] per cut — the overlays of a frame whose main
/// segment did not render — each carrying the frame's classification of it
/// (`service`, one per cut in order; [`CutServiceability::Servable`] past its
/// end).
pub(crate) fn skipped_cuts(
    cuts: &[OverlayCut<'_>],
    service: &[CutServiceability],
) -> Vec<OverlayCutOutcome> {
    cuts.iter()
        .enumerate()
        .map(|(index, cut)| OverlayCutOutcome {
            overlay_id: cut.overlay_id,
            result: Ok(OverlayFrame::Skipped),
            serviceability: service.get(index).copied().unwrap_or_default(),
        })
        .collect()
}

/// Closes every cut's recorded frame after the encoder was submitted (or
/// dropped) — [`OverlaySurface::end_frame`] on each.
pub(crate) fn end_cuts(cuts: &mut [OverlayCut<'_>], submitted: bool) {
    for cut in cuts {
        cut.overlay.end_frame(submitted);
    }
}

/// Drives `future` to completion on the calling thread. Surface configuration
/// is `async` only because device creation can be; the overlay never creates a
/// device (it refused above when there was none), so this resolves without an
/// executor driving it.
fn pollster_block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};

    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_overlay_types_cross_threads() {
        fn send<T: Send>() {}
        send::<OverlaySurface>();
        send::<OverlayRenderer>();
        send::<OverlayError>();
    }

    #[test]
    fn spike_cut_is_past_the_first_clear() {
        let mut scene = frust_scene::Scene::new();
        assert_eq!(spike_cut(&scene), None);
        let mut builder = frust_scene::SceneBuilder::new(&mut scene);
        builder.fill_rect(
            kurbo::Rect::new(0.0, 0.0, 1.0, 1.0),
            peniko::Brush::Solid(peniko::Color::BLACK),
        );
        builder.clear_rect(kurbo::Rect::ZERO);
        builder.clear_rect(kurbo::Rect::ZERO);
        assert_eq!(spike_cut(&scene), Some(2));
    }

    #[test]
    fn the_spike_reads_off_by_default() {
        // The test process sets no `FRUST_OVERLAY_SPIKE`; a build that baked
        // one in is the operator's own A/B, not this suite's.
        if option_env!("FRUST_OVERLAY_SPIKE").is_none()
            && std::env::var(OVERLAY_SPIKE_ENV_VAR).is_err()
        {
            assert!(!overlay_spike_enabled());
            assert_eq!(overlay_spike_value(), None);
        }
    }
}
