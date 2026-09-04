//! Acquiring the GPU, and the contract for what happens when it is not there.
//!
//! # The two halves of the seam
//!
//! Reaching the GPU from a design-system plugin is two separate questions,
//! and the facade answers them with two separate mechanisms:
//!
//! - **Is there a device at all?** `frust::gpu::with_context` reads back the
//!   `DeviceHandle` the running shell installed when its first surface came
//!   up. It answers `None` before that — and, today, always on Android and
//!   iOS (`docs/LIMITATIONS.md`'s `facade-gpu-context-desktop-only`). It is a
//!   lock-free read, safe to call from the UI thread every paint.
//! - **Where does my GPU work go?** `frust::gpu::register_external_pass`
//!   claims a `SceneTextureId` and hands the engine a pass it calls once per
//!   frame with that frame's own device, queue and encoder. The pass — not
//!   this module — is what actually touches the GPU, and it is handed a live
//!   device each time rather than holding one.
//!
//! That split is why [`GpuFxHandle`] stores no `wgpu` handle of any kind. The
//! shell owns the real device and may drop and recreate it (surface loss, a
//! GPU reset); a clone kept past a frame would reference a device that looks
//! alive but backs no future frame. Everything device-shaped lives inside
//! [`crate::gpu_fx::schedule::FxPass`], reached only from `record`.
//!
//! # Graceful degrade
//!
//! [`GpuFx::try_acquire`] answers `None` rather than failing, and a component
//! that gets `None` paints its ordinary 2D path. That is the contract every
//! variant built on this substrate is under: **the 3D path is an
//! enhancement, never the only way a component renders.** The three reasons
//! it can be unavailable are named by [`FxAvailability`] so a component (or a
//! developer reading a log) can tell "this build has it switched off" from
//! "this platform has no device yet".
//!
//! Acquisition is deliberately gated on the *device* being reachable, which
//! is stricter than the pass registry itself requires — the engine drains
//! registered passes on every engine-tier frame path, mobile shells included.
//! Requiring the handle keeps a component from registering a pass on a
//! platform where nothing here has ever been exercised
//! (`docs/LIMITATIONS.md`'s `facade-external-pass-desktop-only`); lifting it
//! is one condition here once a mobile shell installs a handle.
//!
//! # Teardown is explicit
//!
//! [`GpuFxHandle::release`] withdraws the registration, and a component calls
//! it from `View::teardown` — never from a hand-rolled `Drop`, per
//! `docs/CODE_STANDARDS.md`'s State & Reactivity rule (drop order across a
//! component's state/element/owner triple is not a contract; `teardown` is).
//! A handle dropped without `release` leaves its pass registered and called
//! every frame, so this is a real obligation, not a tidiness note.

use std::sync::Arc;

use frust::gpu::{SceneTextureId, register_external_pass, unregister_external_pass};

use super::pool::FxComponentId;
use super::quad3d::Quad3dScene;
use super::schedule::FxPass;

/// The environment variable that turns this catalog's whole GPU-effect path
/// off, leaving every 3D component on its 2D fallback.
///
/// Deliberately its own switch rather than the engine's
/// `FRUST_ENGINE_NO_SHADER_EFFECTS`: that one governs the engine's offscreen
/// *shader-program* path (what [`crate::components::shader_background`]
/// reads), and this one governs caller-registered 3D passes. A driver can
/// fail one and not the other, and an operator turning off a hero background
/// should not silently flatten every card in the catalog. Accepts `1` or
/// `true`, case-insensitively — the engine's own spelling.
pub const KILL_SWITCH_ENV_VAR: &str = "FRUST_BEUI_NO_GPU_FX";

/// Why the 3D path is or is not available this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FxAvailability {
    /// A device is installed and a pass can be registered.
    Ready,
    /// [`KILL_SWITCH_ENV_VAR`] is set: the path is off by operator choice.
    Disabled,
    /// No shell has installed a `DeviceHandle` yet — before the first
    /// surface, or on a platform whose shell does not install one
    /// (`docs/LIMITATIONS.md`'s `facade-gpu-context-desktop-only`).
    NoDevice,
}

impl FxAvailability {
    /// Whether the 3D path can be used.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// The entry point into the substrate: one associated function, and the
/// availability answer behind it.
///
/// A unit type rather than a value, because there is nothing to hold: the
/// device belongs to the shell and the per-component state belongs to the
/// handle this hands back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuFx;

impl GpuFx {
    /// Why the 3D path is or is not available, without claiming anything.
    ///
    /// Cheap enough to call every paint: the kill switch is read from the
    /// environment and the device slot is a lock-free `OnceLock` read.
    #[must_use]
    pub fn availability() -> FxAvailability {
        if disabled() {
            return FxAvailability::Disabled;
        }
        match frust::gpu::with_context(|_| ()) {
            Some(()) => FxAvailability::Ready,
            None => FxAvailability::NoDevice,
        }
    }

    /// Claims a `SceneTextureId` and registers a pass under it, or answers
    /// `None` when the 3D path is unavailable.
    ///
    /// `label` names this component in adapter and validation diagnostics —
    /// a short static string like `"tilt-card"`, not per-instance text.
    ///
    /// A component calls this once (typically on its first paint, since the
    /// shell's device is not installed at construction time) and keeps the
    /// handle. Re-calling after a `None` is fine and is how a component
    /// picks the path up once the first surface exists; re-calling after a
    /// `Some` mints a second id and a second pass, which is a leak, so keep
    /// the handle.
    ///
    /// The `None` cases are exactly [`FxAvailability`]'s two non-ready
    /// variants, plus the vanishingly unlikely one where the freshly minted
    /// id is already claimed — a registration is a claim, never a silent
    /// takeover, so a refused claim is reported rather than forced.
    #[must_use]
    pub fn try_acquire(label: &str) -> Option<GpuFxHandle> {
        if !Self::availability().is_ready() {
            return None;
        }
        let id = SceneTextureId::mint();
        let pass = Arc::new(FxPass::new(id, FxComponentId::mint(), label));
        if !register_external_pass(id, pass.clone()) {
            return None;
        }
        Some(GpuFxHandle { id, pass })
    }
}

/// One component's live claim on the 3D path.
///
/// Holds no GPU resource: an id, and the pass the engine calls every frame.
/// See the module docs for why, and for the explicit-teardown obligation.
#[derive(Debug)]
pub struct GpuFxHandle {
    id: SceneTextureId,
    pass: Arc<FxPass>,
}

impl GpuFxHandle {
    /// The id this component's output is bound under.
    ///
    /// The value a display list names to composite the rendered texture. An
    /// id nothing has bound yet simply draws nothing — never an error — so a
    /// component may record it from its very first paint.
    #[must_use]
    pub const fn id(&self) -> SceneTextureId {
        self.id
    }

    /// [`Self::id`] as the raw `u64` a scene command carries.
    #[must_use]
    pub const fn scene_texture_id(&self) -> u64 {
        self.id.get()
    }

    /// Hands this paint's 3D content to the pass.
    ///
    /// `extent` is the destination rectangle's size in device texels — what
    /// the faces are rendered at, and the extent the engine maps the
    /// destination onto. Call it every paint that has 3D content; the last
    /// scene submitted keeps rendering until it is replaced or
    /// [`Self::clear`]ed, so a frame drained without a repaint still shows
    /// the component rather than blinking out.
    pub fn submit(&self, extent: (u32, u32), scene: Quad3dScene) {
        self.pass.submit(extent, scene);
    }

    /// Withdraws the content without giving up the registration — what a
    /// component calls when it drops back to its 2D path mid-life (reduced
    /// motion switching on, a tilt settling to rest).
    ///
    /// The engine's binding is cleared on the next frame that actually
    /// drains, not immediately: unbinding is not a barrier, and nothing here
    /// forces a frame.
    pub fn clear(&self) {
        self.pass.clear();
    }

    /// Whether a scene is currently waiting to be rendered.
    #[must_use]
    pub fn has_content(&self) -> bool {
        self.pass.has_content()
    }

    /// How many pooled targets this component is holding.
    #[must_use]
    pub fn resident_targets(&self) -> usize {
        self.pass.resident_targets()
    }

    /// Withdraws the registration. Call from `View::teardown`.
    ///
    /// The pass stops being called from the next drain that *starts* after
    /// this returns; one already mid-flight may still call it once more. The
    /// engine's binding for the id is cleared on the next drained frame,
    /// which is not necessarily the next instant — see the module docs.
    pub fn release(self) {
        unregister_external_pass(self.id);
    }
}

/// Whether [`KILL_SWITCH_ENV_VAR`] is set to an accepted true value.
fn disabled() -> bool {
    std::env::var(KILL_SWITCH_ENV_VAR).is_ok_and(|value| {
        let value = value.trim();
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

#[cfg(test)]
mod tests {
    use super::{FxAvailability, GpuFx};

    /// No shell installs a `DeviceHandle` in a test process, so acquisition
    /// answers the fallback case — which is also the contract every component
    /// built on this substrate relies on: `None` is normal, not an error.
    #[test]
    fn acquisition_degrades_when_no_shell_device_exists() {
        assert!(!GpuFx::availability().is_ready());
        assert!(GpuFx::try_acquire("test").is_none());
    }

    #[test]
    fn only_the_ready_variant_reports_ready() {
        assert!(FxAvailability::Ready.is_ready());
        assert!(!FxAvailability::Disabled.is_ready());
        assert!(!FxAvailability::NoDevice.is_ready());
    }

    /// The availability answer names *why*, which is what a developer reads
    /// when a card renders flat. A test process has no device installed, so
    /// the reason is `NoDevice` — unless the run itself set the kill switch,
    /// in which case that answer outranks it and is equally correct.
    #[test]
    fn availability_names_the_reason_it_is_unavailable() {
        let expected = if super::disabled() {
            FxAvailability::Disabled
        } else {
            FxAvailability::NoDevice
        };
        assert_eq!(GpuFx::availability(), expected);
    }
}
