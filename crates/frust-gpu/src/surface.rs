//! Swapchain surfaces: creation, cross-thread hand-off, alpha-mode policy and
//! configuration.
//!
//! Three types cover the whole life of a swapchain, in the order a shell meets
//! them:
//!
//! 1. [`SurfaceFactory`] — a cheap clone of the `wgpu::Instance` that creates a
//!    surface **on the thread the windowing backend requires** (the main/UI
//!    thread on winit/AppKit).
//! 2. [`DetachedSurface`] — the created-but-unconfigured surface, opaque and
//!    `Send`, moved to whichever thread owns the renderer.
//! 3. [`ConfiguredSurface`] — the surface plus the swapchain configuration it
//!    was brought up with, and the one derived answer everything downstream
//!    keys off: whether it *actually* came up translucent
//!    ([`ConfiguredSurface::resolved_translucent`]).
//!
//! Every decision this module makes — which composite alpha mode to resolve
//! ([`resolve_alpha_mode`]), which swapchain format to pick
//! ([`select_surface_format`]), what the configuration should be
//! ([`surface_config`]) — is a pure function over plain values, so it is
//! host-testable with neither a GPU nor a window server. Only
//! [`ConfiguredSurface::configure`]/[`ConfiguredSurface::reconfigure`]/
//! [`ConfiguredSurface::resize`] touch a live device.
//!
//! # Device seam
//!
//! This module speaks `wgpu` primitives directly — a `&wgpu::Instance` to
//! create surfaces from, a `&wgpu::Device` to configure them against — rather
//! than an owning context type. Anything that pools the instance/adapter/device
//! sits above these entry points and passes its handles in, so nothing here
//! needs to know how that pooling works.
//!
//! # Render-attachment only
//!
//! The swapchain is configured with `RENDER_ATTACHMENT` and nothing else: the
//! engine draws the frame through an ordinary render pass, in whichever of
//! [`SURFACE_FORMATS`] the platform reports first. There is no
//! compute-storage-write path here, so no surface has to carry
//! `STORAGE_BINDING` and no surface is pinned to a single format to satisfy a
//! compute target.

use anyhow::{Result, anyhow};

/// Every swapchain format [`select_surface_format`] will configure a surface
/// with — a membership set, not a preference order.
///
/// Both are 8-bit-per-channel unorm formats the engine's pipelines are built
/// for, and the engine warms itself for whichever one the surface hands over,
/// so neither is privileged. Which of the two a platform reporting both lands
/// on is the *surface's* own preference order — see [`select_surface_format`].
pub const SURFACE_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Bgra8Unorm,
];

/// How many frames the swapchain may have in flight before `get_current_texture`
/// blocks. Two is wgpu's own default and the value every frust surface has
/// shipped with: one frame being presented while the next is recorded.
const DESIRED_MAXIMUM_FRAME_LATENCY: u32 = 2;

/// What a caller wants of the surface's alpha, expressed without naming a
/// `wgpu` type — the public, platform-independent request a shell makes.
///
/// **This is a request, not an outcome.** A `TranslucentPreferred` surface on a
/// platform that advertises no translucent composite mode silently resolves
/// opaque; read [`ConfiguredSurface::resolved_translucent`] after configuration
/// rather than keying a paint contract off the request, or a hole-punched view
/// presents black rectangles on an opaque swapchain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceAlphaRequest {
    /// An ordinary opaque window: `wgpu::CompositeAlphaMode::Auto`, whatever the
    /// surface reports.
    Opaque,
    /// Prefer a translucent alpha-compositing mode, so content behind the
    /// surface shows through where the frame's alpha is below 1 — the mode a
    /// hole-punched native view underneath the frame needs.
    TranslucentPreferred,
}

/// Resolves a caller's [`SurfaceAlphaRequest`] against the live surface's
/// reported `alpha_modes`, choosing the actual `wgpu::CompositeAlphaMode` to
/// configure with.
///
/// `Opaque` resolves to `Auto` without consulting the surface at all.
/// `TranslucentPreferred` tries, in order, `Inherit` (Android's only reported
/// translucent mode), `PostMultiplied` (iOS's translucent mode), then
/// `PreMultiplied` — falling back to `Auto` with a `log::warn!` when none of the
/// three is in `capabilities.alpha_modes` (translucency is simply unavailable
/// on that surface).
pub fn resolve_alpha_mode(
    request: SurfaceAlphaRequest,
    capabilities: &wgpu::SurfaceCapabilities,
) -> wgpu::CompositeAlphaMode {
    match request {
        SurfaceAlphaRequest::Opaque => wgpu::CompositeAlphaMode::Auto,
        SurfaceAlphaRequest::TranslucentPreferred => {
            const PREFERRED: [wgpu::CompositeAlphaMode; 3] = [
                wgpu::CompositeAlphaMode::Inherit,
                wgpu::CompositeAlphaMode::PostMultiplied,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ];
            PREFERRED
                .into_iter()
                .find(|mode| capabilities.alpha_modes.contains(mode))
                .unwrap_or_else(|| {
                    log::warn!(
                        "frust-gpu: translucency requested but unavailable \
                         (alpha_modes={:?}) — falling back to Auto (opaque)",
                        capabilities.alpha_modes
                    );
                    wgpu::CompositeAlphaMode::Auto
                })
        }
    }
}

/// Whether a **resolved** `wgpu::CompositeAlphaMode` actually composites the
/// surface's alpha against what is behind it — i.e. whether the surface really
/// came up translucent, as opposed to what the caller *requested*.
///
/// This is the truth behind [`ConfiguredSurface::resolved_translucent`]:
/// [`resolve_alpha_mode`] can silently degrade a
/// [`SurfaceAlphaRequest::TranslucentPreferred`] to `Auto` when the platform
/// advertises no translucent mode, and a shell that kept keying its paint
/// contract off the *request* would then clear to transparent and punch its
/// native-view slots against an OPAQUE swapchain — presenting black rectangles.
///
/// The three translucent modes are exactly [`resolve_alpha_mode`]'s preference
/// list — `Inherit` (Android), `PostMultiplied` (iOS), `PreMultiplied`;
/// `Opaque`/`Auto` ignore the surface's alpha entirely and are therefore *not*
/// translucent (`Auto` is what every opaque caller and every fallback resolves
/// to).
pub fn alpha_mode_is_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::Inherit
            | wgpu::CompositeAlphaMode::PostMultiplied
            | wgpu::CompositeAlphaMode::PreMultiplied
    )
}

/// Whether a swapchain configured with this composite alpha mode expects the
/// pixels it is handed to be **premultiplied**.
///
/// The engine writes premultiplied alpha, so this predicate now reads as "no
/// conversion needed": the two premultiplied-expecting modes are
/// `PreMultiplied` and — the shipped Android translucent case — `Inherit`, and
/// both take the engine's output unchanged. On Android the only reported
/// translucent mode is `Inherit`, under which SurfaceFlinger blends a
/// `TRANSLUCENT` SurfaceView premultiplied; a straight-alpha frame handed to it
/// over-brightens every partial-alpha pixel by `1/a` (measured on device: a
/// 50%-alpha `#FFF176` reaching SurfaceFlinger stored straight `#FFF176@128`
/// instead of premultiplied `#807B3B@128`).
///
/// The cost has therefore moved to the *other* side. `PostMultiplied` — iOS's
/// translucent mode, and the one mode this returns `false` for while still
/// being translucent ([`alpha_mode_is_straight_translucent`]) — expects
/// STRAIGHT alpha, so a translucent iOS surface is the case that needs an
/// un-premultiply pass over the engine's output before present.
/// `Opaque`/`Auto` ignore alpha entirely and need nothing either way.
pub fn alpha_mode_needs_premultiply(mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::Inherit
    )
}

/// Whether a **resolved** alpha mode means "translucent, and the swapchain
/// stores STRAIGHT alpha" — the combination the engine's premultiplied output
/// cannot be presented into unconverted.
///
/// True for `PostMultiplied` alone: it is translucent
/// ([`alpha_mode_is_translucent`]) and does not expect premultiplied pixels
/// ([`alpha_mode_needs_premultiply`] is false), which is iOS's translucent
/// mode. `Inherit`/`PreMultiplied` are translucent *and* premultiplied, so they
/// take the frame as it is written; `Opaque`/`Auto` ignore alpha entirely.
///
/// Pure and mode-only, so the decision is host-testable without a surface; the
/// live per-surface answer is
/// [`ConfiguredSurface::straight_alpha_translucent`], which also respects a
/// translucency the surface never actually got.
pub fn alpha_mode_is_straight_translucent(mode: wgpu::CompositeAlphaMode) -> bool {
    alpha_mode_is_translucent(mode) && !alpha_mode_needs_premultiply(mode)
}

/// Whether `backend`'s compositor actually composites `mode` premultiplied,
/// **despite** [`alpha_mode_is_straight_translucent`] answering true for it —
/// an upstream wgpu-hal truth bug specific to one backend/mode pair, not a
/// property of the mode alone.
///
/// True for `wgpu::Backend::Metal` with `CompositeAlphaMode::PostMultiplied`
/// alone. wgpu-hal's Metal adapter advertises `PostMultiplied`
/// (`wgpu-hal-29.0.4/src/metal/adapter.rs:422-425`,
/// `composite_alpha_modes: [Opaque, PostMultiplied]`) but its surface
/// configuration implements the mode as nothing beyond
/// `render_layer.setOpaque(false)` (`wgpu-hal-29.0.4/src/metal/surface.rs:81-85`)
/// — it never asks Core Animation to treat the layer's content as straight
/// alpha, and Core Animation has no such mode: a `CAMetalLayer` ONLY
/// composites premultiplied (MoltenVK's own equivalent exposes
/// `OPAQUE | PRE_MULTIPLIED` for the identical layer, never a straight
/// option). So a Metal `PostMultiplied` swapchain reads back exactly like a
/// premultiplied one — `display = C·a + BG·(1−a)` — even though the mode's
/// name and wgpu's advertised contract say straight. Handing it the
/// spec-correct straight-alpha conversion therefore double-corrects: the
/// frame is un-premultiplied for a compositor that was going to
/// premultiply-composite it anyway, over-brightening every partial-alpha
/// pixel — device-visible only at fractional alpha (an indigo/navy wash,
/// black frames near a translucent split). See `docs/LIMITATIONS.md`'s
/// `engine-metal-postmultiplied-truth-bug`, and re-check this against
/// wgpu-hal 30's adapter/surface when that pin lands.
///
/// Every other backend keeps the ordinary reading: a genuinely-straight
/// `PostMultiplied` compositor exists on at least one other backend (e.g.
/// Vulkan's own `POST_MULTIPLIED` composite-alpha flag), so this predicate is
/// Metal-specific rather than blanket-disbelieving the mode everywhere.
///
/// A **platform** fact rather than a renderer policy, which is why it sits
/// beside [`resolve_alpha_mode`] here: which render path a renderer picks in
/// response is the renderer's own business (see
/// `frust_render::context::choose_engine_render_path`, the one caller).
///
/// Pure and (backend, mode)-only, so the decision is host-testable without a
/// live adapter.
pub fn compositor_expects_premultiplied(
    backend: wgpu::Backend,
    mode: wgpu::CompositeAlphaMode,
) -> bool {
    backend == wgpu::Backend::Metal && mode == wgpu::CompositeAlphaMode::PostMultiplied
}

/// Picks the swapchain format to configure with: the first format **the
/// surface reports** that is one of [`SURFACE_FORMATS`].
///
/// The preference order is the *surface's*, not this module's — the platform
/// lists its formats best-first, and taking its first supported entry is the
/// selection every shipped frust build has configured its swapchain with
/// (device-gated on Android/Vulkan, iOS/macOS/Metal and Windows). The engine
/// warms its strip pipelines for whichever of the two it is handed, so neither
/// is privileged here; reordering this to prefer `Rgba8Unorm` would silently
/// change the swapchain format on the platforms that report `Bgra8Unorm` first.
///
/// Errors when the surface supports neither, which no shipping platform does —
/// the engine draws through a render pass into whichever of the two is
/// available and has no third format to fall back on.
pub fn select_surface_format(
    capabilities: &wgpu::SurfaceCapabilities,
) -> Result<wgpu::TextureFormat> {
    capabilities
        .formats
        .iter()
        .copied()
        .find(|format| SURFACE_FORMATS.contains(format))
        .ok_or_else(|| {
            anyhow!(
                "frust-gpu: no supported surface format (Rgba8Unorm/Bgra8Unorm) in {:?}",
                capabilities.formats
            )
        })
}

/// Builds the `wgpu::SurfaceConfiguration` a surface is brought up with.
///
/// Split out of [`ConfiguredSurface::configure`] so the configuration a surface
/// gets is decided by a pure function over plain values and can be asserted
/// without a device: `RENDER_ATTACHMENT` usage only, the caller's format and
/// alpha mode verbatim, no view formats, and the fixed frame latency
/// (`DESIRED_MAXIMUM_FRAME_LATENCY`, two frames in flight).
///
/// `size` is `(width, height)` in physical pixels and must be non-zero on both
/// axes; a zero-sized swapchain is a `wgpu` validation error, and the shell's
/// own resize path is where a minimized/zero-extent window is filtered out.
pub fn surface_config(
    format: wgpu::TextureFormat,
    alpha_mode: wgpu::CompositeAlphaMode,
    size: (u32, u32),
    present_mode: wgpu::PresentMode,
) -> wgpu::SurfaceConfiguration {
    let (width, height) = size;
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode,
        desired_maximum_frame_latency: DESIRED_MAXIMUM_FRAME_LATENCY,
        alpha_mode,
        view_formats: vec![],
    }
}

/// A cheap, cloneable handle to a `wgpu::Instance` used to create a surface on
/// a *different* thread than the one that owns the renderer.
///
/// # Why this exists
///
/// `wgpu` surface creation reads the platform window handle, which several
/// windowing backends (notably winit on macOS/AppKit) only make available on
/// the main/UI thread. A render-thread split therefore cannot create the
/// surface where the renderer lives; instead the UI thread creates a
/// [`DetachedSurface`] via this factory and hands it across to the render
/// thread, which configures it there. A `wgpu::Instance` is `Send + Sync +
/// Clone` (Arc-backed) and a `Surface` it produces stays compatible with any
/// adapter/device the cloned instance requests, so the two threads share one
/// instance with no `unsafe`.
///
/// The mobile shells do not need this: they receive a platform-created surface
/// pointer and go through
/// [`create_android_surface`](crate::lifecycle::create_android_surface) /
/// `create_metal_surface` instead.
#[derive(Clone)]
pub struct SurfaceFactory {
    instance: wgpu::Instance,
}

impl SurfaceFactory {
    /// Clones `instance` into a factory that can be moved to the UI thread.
    pub fn new(instance: &wgpu::Instance) -> Self {
        Self {
            instance: instance.clone(),
        }
    }

    /// Create a [`DetachedSurface`] from a window handle **on the calling
    /// thread** — call this on the thread the windowing backend requires (the
    /// main/UI thread for winit). The returned surface is `Send` and may then be
    /// moved to the render thread and configured there.
    ///
    /// This performs *only* the window-handle-dependent step (surface creation);
    /// the device and the swapchain configuration are both built later, on the
    /// configuring thread, so nothing here touches a GPU device.
    pub fn create_detached_surface(
        &self,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<DetachedSurface> {
        let surface = self
            .instance
            .create_surface(target)
            .map_err(|e| anyhow!("frust-gpu: failed to create surface: {e}"))?;
        Ok(DetachedSurface { surface })
    }
}

/// A created-but-not-yet-configured `wgpu::Surface`, produced by
/// [`SurfaceFactory::create_detached_surface`] on the windowing thread and
/// configured on the render thread.
///
/// Opaque on purpose: a shell only ever moves this value across a thread
/// boundary, so the wrapped `wgpu` type stays out of every signature above this
/// crate. `Send`, which is the whole point.
pub struct DetachedSurface {
    surface: wgpu::Surface<'static>,
}

impl DetachedSurface {
    /// Consume the wrapper, yielding the raw surface to configure.
    ///
    /// Hidden from the rendered docs rather than made private: the renderer
    /// crate above this one is the intended (and only) caller, and no layer
    /// above *it* may re-export this accessor — doing so would make the wrapped
    /// `wgpu::Surface` nameable from shell and widget code, which is exactly
    /// what the wrapper exists to prevent.
    #[doc(hidden)]
    pub fn into_surface(self) -> wgpu::Surface<'static> {
        self.surface
    }
}

/// A configured swapchain surface: the surface itself, the configuration it was
/// brought up with, and whether it actually came up translucent.
pub struct ConfiguredSurface {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Whether this surface **actually** came up translucent — the projection of
    /// `config.alpha_mode` through [`alpha_mode_is_translucent`], computed once
    /// at configure time (the mode never changes for a live surface; a resize
    /// reconfigures with the same mode).
    ///
    /// Stored as a plain `bool` rather than re-derived from `config.alpha_mode`
    /// at each read, so the value a shell observes crosses this crate's boundary
    /// with no `wgpu` type in the signature.
    resolved_translucent: bool,
}

impl ConfiguredSurface {
    /// Configures `surface` against `device` and returns the wrapper.
    ///
    /// `format` is the format the caller resolved from what the *surface*
    /// reports ([`select_surface_format`]) rather than one this layer imposes,
    /// and `alpha_mode` the one [`resolve_alpha_mode`] resolved from the
    /// surface's reported modes. `size` is `(width, height)` in physical pixels
    /// and must be non-zero on both axes.
    pub fn configure(
        surface: wgpu::Surface<'static>,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        alpha_mode: wgpu::CompositeAlphaMode,
        size: (u32, u32),
        present_mode: wgpu::PresentMode,
    ) -> Self {
        let config = surface_config(format, alpha_mode, size, present_mode);
        surface.configure(device, &config);
        Self {
            surface,
            config,
            resolved_translucent: alpha_mode_is_translucent(alpha_mode),
        }
    }

    /// Re-applies the current configuration to the swapchain — the recovery step
    /// an `Outdated` (or retried `Invalid`) acquire asks for, where the surface
    /// is stale but its geometry has not changed.
    pub fn reconfigure(&self, device: &wgpu::Device) {
        self.surface.configure(device, &self.config);
    }

    /// Resizes the swapchain in place. `width`/`height` are physical pixels and
    /// must both be non-zero; the shell's resize path filters a minimized or
    /// zero-extent window out before reaching here.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.reconfigure(device);
    }

    /// The live surface, for acquiring a swapchain texture.
    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }

    /// The configuration this surface is currently up with.
    pub fn config(&self) -> &wgpu::SurfaceConfiguration {
        &self.config
    }

    /// The swapchain's `(width, height)` in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Whether this surface really came up translucent — the answer a shell's
    /// paint contract keys off, never the original
    /// [`SurfaceAlphaRequest`].
    pub fn resolved_translucent(&self) -> bool {
        self.resolved_translucent
    }

    /// Whether this surface really came up translucent with a swapchain that
    /// stores STRAIGHT alpha — iOS's `PostMultiplied` translucent mode, the one
    /// configuration whose present needs the engine's premultiplied output
    /// converted back.
    ///
    /// Reads [`Self::resolved_translucent`] rather than the raw alpha mode, so a
    /// surface that never got the translucency it asked for answers `false`.
    pub fn straight_alpha_translucent(&self) -> bool {
        self.resolved_translucent && alpha_mode_is_straight_translucent(self.config.alpha_mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic `wgpu::SurfaceCapabilities` reporting only the given
    /// `alpha_modes` — the rest of the struct is irrelevant to
    /// `resolve_alpha_mode`, which reads only that one field.
    fn caps_with_alpha_modes(
        alpha_modes: &[wgpu::CompositeAlphaMode],
    ) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            alpha_modes: alpha_modes.to_vec(),
            ..Default::default()
        }
    }

    /// The same for the format list `select_surface_format` reads.
    fn caps_with_formats(formats: &[wgpu::TextureFormat]) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            formats: formats.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn opaque_request_always_resolves_to_auto() {
        // `Opaque` never consults `alpha_modes` — the same answer regardless of
        // what the surface reports.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
            &[wgpu::CompositeAlphaMode::Opaque],
        ] {
            let caps = caps_with_alpha_modes(modes);
            assert_eq!(
                resolve_alpha_mode(SurfaceAlphaRequest::Opaque, &caps),
                wgpu::CompositeAlphaMode::Auto
            );
        }
    }

    #[test]
    fn translucent_preferred_picks_inherit_first() {
        // Android's observed shape: `Inherit` is the only reported mode.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Inherit
        );
    }

    #[test]
    fn translucent_preferred_picks_post_multiplied_when_inherit_absent() {
        // iOS's observed shape: `[Opaque, PostMultiplied]` — no `Inherit`.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PostMultiplied
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_pre_multiplied_last() {
        // Neither `Inherit` nor `PostMultiplied` present, but `PreMultiplied`
        // is — the third preference in the resolution order.
        let caps = caps_with_alpha_modes(&[
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
    }

    #[test]
    fn translucent_preferred_falls_back_to_auto_when_none_available() {
        // A surface reporting only `Opaque` (no `Inherit`/`PostMultiplied`/
        // `PreMultiplied`) can't satisfy translucency — fall back to `Auto`
        // rather than erroring.
        let caps = caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Opaque]);
        assert_eq!(
            resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps),
            wgpu::CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn resolved_translucency_is_true_only_for_the_three_translucent_modes() {
        // The projection a shell's paint contract keys off: exactly
        // `resolve_alpha_mode`'s preference list.
        for mode in [
            wgpu::CompositeAlphaMode::Inherit,
            wgpu::CompositeAlphaMode::PostMultiplied,
            wgpu::CompositeAlphaMode::PreMultiplied,
        ] {
            assert!(alpha_mode_is_translucent(mode), "{mode:?}");
        }
        // `Auto` is what BOTH an opaque request and a failed translucent
        // resolution land on — neither composites alpha.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
        ] {
            assert!(!alpha_mode_is_translucent(mode), "{mode:?}");
        }
    }

    #[test]
    fn premultiplied_expecting_modes_take_engine_output_unchanged() {
        // The engine writes premultiplied alpha, so these two are the modes that
        // need no conversion at all.
        for mode in [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::Inherit,
        ] {
            assert!(alpha_mode_needs_premultiply(mode), "{mode:?}");
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
        // Alpha-ignoring modes never expect premultiplied pixels either.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
        ] {
            assert!(!alpha_mode_needs_premultiply(mode), "{mode:?}");
        }
    }

    /// Over every mode `resolve_alpha_mode` can produce, `PostMultiplied` alone
    /// is translucent AND straight-alpha — the one surface that has to convert
    /// the engine's premultiplied output before present.
    #[test]
    fn only_post_multiplied_is_translucent_with_straight_alpha() {
        assert!(alpha_mode_is_straight_translucent(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        assert!(!alpha_mode_needs_premultiply(
            wgpu::CompositeAlphaMode::PostMultiplied
        ));
        // Opaque destinations are not translucent at all, so they are not the
        // straight-translucent case however their alpha is stored.
        for mode in [
            wgpu::CompositeAlphaMode::Auto,
            wgpu::CompositeAlphaMode::Opaque,
        ] {
            assert!(!alpha_mode_is_straight_translucent(mode), "{mode:?}");
        }
    }

    #[test]
    fn the_surfaces_own_order_decides_when_both_formats_are_reported() {
        // The shipped selection: the platform lists its formats best-first and
        // the first supported entry wins, whichever of the two that is. This is
        // what every device-gated build configured its swapchain with; picking
        // by THIS module's order instead would flip the format on platforms
        // that report `Bgra8Unorm` first.
        let bgra_first = caps_with_formats(&[
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]);
        assert_eq!(
            select_surface_format(&bgra_first).unwrap(),
            wgpu::TextureFormat::Bgra8Unorm,
            "the surface's own preference order decides, not this module's"
        );
        let rgba_first = caps_with_formats(&[
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
        ]);
        assert_eq!(
            select_surface_format(&rgba_first).unwrap(),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }

    #[test]
    fn unsupported_formats_are_skipped_over_rather_than_taken() {
        // The shape a Metal/Dx12 surface reports: an sRGB variant first, which
        // is not one of ours, then the plain `Bgra8Unorm` that is.
        let caps = caps_with_formats(&[
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm,
        ]);
        assert_eq!(
            select_surface_format(&caps).unwrap(),
            wgpu::TextureFormat::Bgra8Unorm
        );
    }

    #[test]
    fn a_surface_supporting_neither_format_is_an_error() {
        let caps = caps_with_formats(&[wgpu::TextureFormat::Rgba16Float]);
        assert!(select_surface_format(&caps).is_err());
    }

    #[test]
    fn config_asks_for_render_attachment_only() {
        // The engine draws the frame through a render pass; nothing here writes
        // the swapchain through a compute storage binding, so no surface has to
        // carry `STORAGE_BINDING`.
        let config = surface_config(
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::CompositeAlphaMode::Auto,
            (800, 600),
            wgpu::PresentMode::AutoVsync,
        );
        assert_eq!(config.usage, wgpu::TextureUsages::RENDER_ATTACHMENT);
    }

    #[test]
    fn config_carries_the_callers_format_alpha_mode_and_size() {
        // Both `SURFACE_FORMATS` entries configure verbatim: the format comes
        // from what the surface reported, not from a fixed engine target format.
        for format in SURFACE_FORMATS {
            let config = surface_config(
                format,
                wgpu::CompositeAlphaMode::Inherit,
                (1080, 2400),
                wgpu::PresentMode::Fifo,
            );
            assert_eq!(config.format, format);
            assert_eq!(config.alpha_mode, wgpu::CompositeAlphaMode::Inherit);
            assert_eq!((config.width, config.height), (1080, 2400));
            assert_eq!(config.present_mode, wgpu::PresentMode::Fifo);
            assert!(config.view_formats.is_empty());
            assert_eq!(
                config.desired_maximum_frame_latency,
                DESIRED_MAXIMUM_FRAME_LATENCY
            );
        }
    }

    #[test]
    fn forced_mismatch_translucent_request_resolves_not_translucent() {
        // A surface whose advertised capabilities carry NO translucent mode,
        // asked for translucency. The request is honoured as far as it can be
        // (`Auto`), but the RESOLVED translucency — the value
        // `SurfaceRenderer::surface_resolved_translucent` hands the shells — is
        // `false`, so the shells keep the opaque (Mode A) paint contract: an
        // opaque base colour and no `ClearRect` punch.
        for modes in [
            [wgpu::CompositeAlphaMode::Opaque].as_slice(),
            &[wgpu::CompositeAlphaMode::Auto],
            &[],
        ] {
            let caps = caps_with_alpha_modes(modes);
            let resolved = resolve_alpha_mode(SurfaceAlphaRequest::TranslucentPreferred, &caps);
            assert_eq!(resolved, wgpu::CompositeAlphaMode::Auto, "{modes:?}");
            assert!(
                !alpha_mode_is_translucent(resolved),
                "a fallback-to-opaque surface must never report translucent ({modes:?})"
            );
        }
    }

    #[test]
    fn happy_path_translucent_request_resolves_translucent() {
        // The shipped configs stay unchanged: Android (`Inherit`-only) and iOS
        // (`[Opaque, PostMultiplied]`) both resolve to a translucent mode, so
        // the punch + transparent base keep running exactly as today.
        for modes in [
            [wgpu::CompositeAlphaMode::Inherit].as_slice(),
            &[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ],
        ] {
            let resolved = resolve_alpha_mode(
                SurfaceAlphaRequest::TranslucentPreferred,
                &caps_with_alpha_modes(modes),
            );
            assert!(alpha_mode_is_translucent(resolved), "{modes:?}");
        }
        // An opaque request never reports translucent, whatever the surface
        // advertises.
        assert!(!alpha_mode_is_translucent(resolve_alpha_mode(
            SurfaceAlphaRequest::Opaque,
            &caps_with_alpha_modes(&[wgpu::CompositeAlphaMode::Inherit]),
        )));
    }

    /// Every composite alpha mode a surface can resolve to, so a claim about
    /// the truth bug is made across the whole space rather than the modes it
    /// was written for.
    const EVERY_ALPHA_MODE: [wgpu::CompositeAlphaMode; 5] = [
        wgpu::CompositeAlphaMode::Auto,
        wgpu::CompositeAlphaMode::Opaque,
        wgpu::CompositeAlphaMode::Inherit,
        wgpu::CompositeAlphaMode::PreMultiplied,
        wgpu::CompositeAlphaMode::PostMultiplied,
    ];

    #[test]
    fn compositor_expects_premultiplied_is_metal_post_multiplied_only() {
        // Direct guard on the predicate itself, across the whole (backend,
        // mode) space: the upstream wgpu-hal truth bug is one backend/mode
        // pair, and must never spread to another backend or another mode.
        for backend in wgpu::Backend::ALL {
            for mode in EVERY_ALPHA_MODE {
                let expected = backend == wgpu::Backend::Metal
                    && mode == wgpu::CompositeAlphaMode::PostMultiplied;
                assert_eq!(
                    compositor_expects_premultiplied(backend, mode),
                    expected,
                    "compositor_expects_premultiplied({backend:?}, {mode:?})"
                );
            }
        }
    }

    #[test]
    fn the_truth_bug_only_ever_contradicts_a_straight_translucent_mode() {
        // The predicate exists to override `alpha_mode_is_straight_translucent`
        // for one pair; anywhere it answers true, that predicate must have
        // answered true too, or it would be silently redirecting a mode that
        // never needed conversion in the first place.
        for backend in wgpu::Backend::ALL {
            for mode in EVERY_ALPHA_MODE {
                if compositor_expects_premultiplied(backend, mode) {
                    assert!(
                        alpha_mode_is_straight_translucent(mode),
                        "{backend:?}/{mode:?} is contradicted without being straight-translucent"
                    );
                }
            }
        }
    }
}
