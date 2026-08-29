//! Instance, adapter and lazy logical-device creation: [`Context`] and
//! [`DeviceHandle`].
//!
//! This is the crate's entry point — everything else in `frust-gpu` consumes a
//! [`DeviceHandle`] rather than reaching for a `wgpu::Adapter` itself. A
//! [`Context`] owns one `wgpu::Instance` and at most one logical device, created
//! lazily on the first [`Context::device`] call and then reused: a logical
//! device is display-independent, so surface loss and recreation (rotation,
//! backgrounding) must not rebuild it.
//!
//! # Pure decision vs. platform lookup
//!
//! Every environment-sensitive choice here is split into a pure function taking
//! the environment as an argument ([`effective_instance_flags`],
//! [`effective_limits`], [`required_features`], [`decide_log_action`]) plus a
//! separate lookup that answers what the environment actually is
//! (`is_android_emulator`, [`is_ios_simulator`]). Only the lookups are
//! platform-gated, so the policies stay unit-testable on any host with no GPU
//! and no mobile target in the loop — the same split
//! [`crate::caps::TierCaps::probe`]/[`crate::caps::TierCaps::fake`] gives
//! adapter capabilities.
//!
//! # Two mitigations worth knowing about
//!
//! - The Android **emulator** cannot survive `wgpu::InstanceFlags::DEBUG`, so
//!   the instance is built with those flags stripped there and nowhere else
//!   ([`effective_instance_flags`]).
//! - The iOS **Simulator** misreports its uniform-buffer alignment, so a device
//!   request made there is forced back up to 256 bytes ([`effective_limits`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{Result, anyhow};

use crate::caps::{DownlevelProfile, TierCaps};

/// Default `wgpu::Device` debug label, used when a caller supplies no
/// [`ContextOptions::device_label`] of its own.
const DEFAULT_DEVICE_LABEL: &str = "frust-gpu device";

/// How a [`Context`] should build its instance and request its device.
///
/// Every field has a working default, so `ContextOptions::default()` is the
/// ordinary construction — a field exists here only where a host genuinely has
/// a choice to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextOptions {
    /// Debug label attached to the created `wgpu::Device`, surfaced by graphics
    /// debuggers and in validation messages.
    pub device_label: String,
    /// Backends the `wgpu::Instance` is restricted to. `None` — the default —
    /// takes `wgpu::Backends::from_env()` (the `WGPU_BACKEND` knob) and falls
    /// back to every backend compiled in, which is what a shell wants. A
    /// headless caller that must pin one backend regardless of the ambient
    /// environment sets it explicitly.
    pub backends: Option<wgpu::Backends>,
}

impl Default for ContextOptions {
    fn default() -> Self {
        Self {
            device_label: DEFAULT_DEVICE_LABEL.to_string(),
            backends: None,
        }
    }
}

/// A logical device, the adapter it was created from, the queue that executes
/// its command buffers, and the capabilities that adapter reported.
///
/// Cheap to clone: `wgpu`'s `Adapter`/`Device`/`Queue` are all `Arc`-backed
/// handles to one underlying object, so a clone is another handle to the *same*
/// device rather than a second device. Clone it freely to hand a subsystem the
/// device it needs instead of threading a `&Context` borrow through it.
#[derive(Clone, Debug)]
pub struct DeviceHandle {
    /// The adapter the device was created from.
    pub adapter: wgpu::Adapter,
    /// The logical device.
    pub device: wgpu::Device,
    /// The queue that executes this device's command buffers.
    pub queue: wgpu::Queue,
    /// What [`Self::adapter`] reported at device-creation time. Captured once
    /// so downstream pipeline/atlas decisions read plain data instead of
    /// re-probing the adapter.
    pub caps: TierCaps,
    /// The first uncaptured error this device raised, latched by the handler
    /// installed in [`Context::device`]. See [`Self::first_uncaptured_error`].
    first_uncaptured_error: Arc<OnceLock<String>>,
}

impl DeviceHandle {
    /// The **first** uncaptured `wgpu` error this device ever raised, or `None`
    /// if it has raised none.
    ///
    /// Latched, never overwritten: a frame loop polling this wants the error
    /// that started the trouble, not the last one in a storm the first one
    /// caused. It is also the only programmatic view of an uncaptured error a
    /// caller gets — the handler otherwise only logs (see
    /// [`decide_log_action`]) — so a host can degrade or report instead of
    /// silently rendering nothing every frame.
    ///
    /// Deliberately not clearable: "this device has seen an uncaptured error"
    /// is a property of the device, and a device that has raised one is not
    /// reliably recoverable by forgetting about it.
    pub fn first_uncaptured_error(&self) -> Option<&str> {
        self.first_uncaptured_error.get().map(String::as_str)
    }
}

/// Owns the `wgpu::Instance` and the single logical device this crate's
/// consumers render with.
///
/// The device is created lazily — [`Context::new`] performs no adapter
/// enumeration at all, so a host may build a `Context` early (before it has a
/// window, or on a thread that will never render) and pay for the device only
/// at the first [`Context::device`] call.
pub struct Context {
    instance: wgpu::Instance,
    options: ContextOptions,
    /// `None` until the first [`Context::device`] call succeeds.
    device: Option<DeviceHandle>,
}

impl Context {
    /// Creates a context with a fresh `wgpu::Instance` and no device yet.
    ///
    /// The instance flags come from the build configuration
    /// (`InstanceFlags::from_build_config`, which turns `DEBUG`/`VALIDATION` on
    /// in debug builds) plus the standard `WGPU_*` environment overrides, and
    /// are then run through [`effective_instance_flags`] on Android so an
    /// emulator gets them stripped.
    pub fn new(options: ContextOptions) -> Self {
        let backends = options
            .backends
            .unwrap_or_else(|| wgpu::Backends::from_env().unwrap_or_default());
        let build_flags = wgpu::InstanceFlags::from_build_config().with_env();
        #[cfg(target_os = "android")]
        let flags = effective_instance_flags(build_flags, is_android_emulator());
        #[cfg(not(target_os = "android"))]
        let flags = build_flags;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends,
            flags,
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            instance,
            options,
            device: None,
        }
    }

    /// The logical device, creating it on first call and returning the same one
    /// afterwards.
    ///
    /// Adapter selection goes through
    /// `wgpu::util::initialize_adapter_from_env_or_default`, so
    /// `WGPU_ADAPTER_NAME`/`WGPU_POWER_PREF` pick the adapter on a host with
    /// more than one — the only way to pin a specific GPU on a multi-adapter
    /// machine.
    ///
    /// # Errors
    ///
    /// When no adapter is available at all, or when the device request the
    /// adapter's own limits were computed for is nonetheless refused. Both are
    /// terminal for GPU rendering; neither is retryable by calling again.
    pub async fn device(&mut self) -> Result<&DeviceHandle> {
        if self.device.is_none() {
            self.device = Some(create_device(&self.instance, &self.options).await?);
        }
        Ok(self
            .device
            .as_ref()
            .expect("device was just created or already present"))
    }

    /// What the live device's adapter reported, or `None` while the device is
    /// still uncreated — capabilities are an adapter's answer, and no adapter
    /// has been selected before the first [`Context::device`] call.
    pub fn caps(&self) -> Option<&TierCaps> {
        self.device.as_ref().map(|handle| &handle.caps)
    }
}

/// Selects an adapter, probes it, and requests the logical device — the body of
/// [`Context::device`]'s lazy arm.
///
/// A free function rather than a method so it borrows the instance and options
/// separately from the `device` field [`Context::device`] is assigning into.
async fn create_device(
    instance: &wgpu::Instance,
    options: &ContextOptions,
) -> Result<DeviceHandle> {
    let adapter = wgpu::util::initialize_adapter_from_env_or_default(instance, None)
        .await
        .map_err(|e| anyhow!("frust-gpu: no compatible GPU adapter: {e}"))?;

    let caps = TierCaps::probe(&adapter);

    // The device request is built from the resolved downlevel profile, not
    // unconditionally from the adapter's raw limits: under
    // `DownlevelProfile::WebGl2` (a real `Gl` backend, or
    // `FRUST_ENGINE_DOWNLEVEL=1` rehearsing it) the request itself must ask
    // for the GLES-3.0/WebGL2 downlevel default shape, or the override would
    // only relabel a full desktop device rather than actually exercising it.
    // `using_resolution` folds in the adapter's own texture-dimension limits
    // so the request never asks for a resolution the adapter cannot satisfy
    // (the swapchain may need more than the downlevel default allows) while
    // every other WebGL2 default limit is requested as-is. Under
    // `DownlevelProfile::Full` this reads the adapter directly rather than
    // going through `caps`: the summarised capabilities carry the individual
    // limits decisions are made from, not the struct `request_device`
    // requires, and feeding the *adapter's* limits (rather than
    // `Limits::default()`) is what keeps the request from over-asking and
    // failing on a constrained mobile adapter.
    let base_limits = if caps.downlevel_profile == DownlevelProfile::WebGl2 {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        adapter.limits()
    };
    let required_limits = effective_limits(base_limits, is_ios_simulator());
    let required_features = required_features(&caps, cfg!(feature = "perf-trace"));

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(&options.device_label),
            required_features,
            required_limits,
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow!("frust-gpu: failed to create GPU device: {e}"))?;

    // Route wgpu's uncaptured errors to the log and a latch instead of its
    // default handler, which panics — "handling wgpu errors as fatal by
    // default". A UI framework must survive a driver's *transient* GPU error
    // and recover on a later frame rather than abort the process, which across
    // a mobile FFI boundary means killing the host app. Genuine API misuse is
    // still surfaced loudly at error level.
    //
    // Two pieces of state, both per-device (captured fresh each time this
    // closure is installed) and both behind `Arc` because the handler must be
    // `Fn`, not `FnMut`:
    //
    // - `error_count` drives the log latch ([`decide_log_action`]), so a device
    //   wedged in a per-frame error storm cannot flood the log forever.
    // - `first_error` latches the first error's text for the frame loop to
    //   poll via [`DeviceHandle::first_uncaptured_error`]. `OnceLock` gives
    //   exactly first-write-wins with no lock held across the handler body.
    let error_count = Arc::new(AtomicU32::new(0));
    let first_error: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let latch = Arc::clone(&first_error);
    device.on_uncaptured_error(Arc::new(move |error| {
        let count = error_count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = latch.set(error.to_string());
        match decide_log_action(count) {
            LogAction::Log => {
                log::error!("frust-gpu: uncaptured wgpu error: {error}");
            }
            LogAction::SuppressionNotice => {
                log::error!(
                    "frust-gpu: further uncaptured wgpu errors suppressed \
                     (total so far: {count})"
                );
            }
            LogAction::Silent { debug_bump } => {
                if debug_bump {
                    log::debug!(
                        "frust-gpu: uncaptured wgpu error count now {count} \
                         (still suppressed)"
                    );
                }
            }
        }
    }));

    Ok(DeviceHandle {
        adapter,
        device,
        queue,
        caps,
        first_uncaptured_error: first_error,
    })
}

/// Given the build-config-derived instance flags and whether the process is
/// currently running on an Android emulator, decides the flags wgpu's
/// `Instance` should actually be created with.
///
/// The `DEBUG` flag makes wgpu enable `VK_EXT_debug_utils` and set object-name
/// labels via `vkSetDebugUtilsObjectNameEXT`, and the emulator's gfxstream
/// Vulkan HAL (`vulkan.ranchu.so`) segfaults inside that entry point during
/// adapter enumeration (observed crash: `#00 vulkan.ranchu.so
/// vk_common_SetDebugUtilsObjectNameEXT`) — the same class of debug-utils
/// fragility a MoltenVK Vulkan backend is also known to have. Debug object
/// labels are only a developer convenience, so dropping them on the emulator
/// keeps GPU bring-up alive there while leaving physical devices' validation
/// safety net — and desktop behaviour — untouched.
///
/// Pure decision logic, kept separate from the platform property lookup in
/// `is_android_emulator` so it is unit-testable on any host without an Android
/// target.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn effective_instance_flags(flags: wgpu::InstanceFlags, is_emulator: bool) -> wgpu::InstanceFlags {
    if is_emulator {
        flags - (wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION)
    } else {
        flags
    }
}

/// Detects whether the current process is running on an Android emulator
/// (goldfish/ranchu), as opposed to a physical device, via the standard
/// `ro.kernel.qemu` system property (`"1"` on emulators, unset/absent on real
/// hardware). A failed property read is treated as "not an emulator" so
/// physical devices — and any environment where the property cannot be read —
/// default to keeping validation on.
#[cfg(target_os = "android")]
fn is_android_emulator() -> bool {
    android_system_properties::AndroidSystemProperties::new()
        .get("ro.kernel.qemu")
        .as_deref()
        == Some("1")
}

/// The uniform-buffer offset alignment the iOS Simulator's Metal validation
/// actually enforces, regardless of what the adapter reports.
const IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT: u32 = 256;

/// Given a base `wgpu::Limits` and whether the process is currently running on
/// an iOS Simulator, decides the `Limits` a device request should actually use.
///
/// Mitigates [wgpu #7057](https://github.com/gfx-rs/wgpu/issues/7057): the iOS
/// Simulator is macOS-Metal-backed and requires 256-byte
/// `min_uniform_buffer_offset_alignment`, but wgpu's Metal backend reports the
/// (lower) iOS-device value, which trips Metal API validation on the simulator.
/// Physical iOS devices are unaffected and pass `base` through unchanged; a
/// `base` whose alignment is already at or above 256 is left alone, never
/// lowered.
///
/// Upstream [gfx-rs/wgpu PR #10189](https://github.com/gfx-rs/wgpu/pull/10189)
/// makes this unnecessary — drop it once a pinned wgpu release contains it.
///
/// Pure decision logic, mirroring [`effective_instance_flags`]'s split of pure
/// decision vs. platform lookup. It is fed the profile-resolved base limits
/// (see [`create_device`]) — the adapter's own limits under
/// `DownlevelProfile::Full`, the WebGL2 downlevel defaults resolution-folded
/// with the adapter otherwise — so the device request never over-asks.
fn effective_limits(base: wgpu::Limits, is_ios_simulator: bool) -> wgpu::Limits {
    if is_ios_simulator
        && base.min_uniform_buffer_offset_alignment
            < IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT
    {
        wgpu::Limits {
            min_uniform_buffer_offset_alignment: IOS_SIMULATOR_MIN_UNIFORM_BUFFER_OFFSET_ALIGNMENT,
            ..base
        }
    } else {
        base
    }
}

/// Whether this binary is running on the iOS Simulator
/// (`aarch64-apple-ios-sim` / `x86_64-apple-ios` under the simulator), which
/// sets `target_abi = "sim"`. Compile-time constant: the simulator mitigation
/// only needs to apply to simulator builds, never physical-device or desktop
/// ones.
const fn is_ios_simulator() -> bool {
    cfg!(all(target_os = "ios", target_abi = "sim"))
}

/// The `wgpu::Features` a device request should ask for, given what the adapter
/// offers and whether this build compiled the `perf-trace` feature in.
///
/// The policy is deliberately minimal: **empty** by default. A required feature
/// is a hard device-creation failure on any adapter lacking it, so asking for
/// something the crate does not actually need converts a working device into no
/// device at all. `TIMESTAMP_QUERY` is the single exception — it is what GPU
/// timing probes are built on, so a `perf-trace` build asks for it, and even
/// then only when the adapter offers it, so enabling the feature can never turn
/// a working adapter into a failed device request.
fn required_features(caps: &TierCaps, perf_trace: bool) -> wgpu::Features {
    if perf_trace && caps.has_timestamp_query {
        wgpu::Features::TIMESTAMP_QUERY
    } else {
        wgpu::Features::empty()
    }
}

/// Number of uncaptured `wgpu` errors logged at error level per device before
/// the handler latches into suppression. A single flaky frame under a driver
/// hiccup (e.g. the Android emulator's SwiftShader path) is expected to surface
/// a handful of errors; past this the process is either wedged in a genuine
/// per-frame error storm or the driver is fundamentally broken, and re-logging
/// every single one would flood the log without adding information.
const MAX_LOGGED_UNCAPTURED_ERRORS: u32 = 5;

/// How often (in error count) a latched handler bumps a debug-level "still
/// happening" line once past [`MAX_LOGGED_UNCAPTURED_ERRORS`] and the one
/// suppression notice. Debug level (not error) because this is diagnostic noise
/// for someone actively investigating, not an actionable signal.
const UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD: u32 = 100;

/// What the uncaptured-error handler should do for the `count`-th uncaptured
/// error (1-indexed) it has observed on a given device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogAction {
    /// One of the first [`MAX_LOGGED_UNCAPTURED_ERRORS`]: log the error itself
    /// at error level.
    Log,
    /// The first error past the cap: log one suppression notice (naming the
    /// running total) instead of the error itself.
    SuppressionNotice,
    /// Past the cap and past the suppression notice: stay silent, except a
    /// periodic debug-level count bump when `debug_bump` is set.
    Silent { debug_bump: bool },
}

/// Pure latch policy for the uncaptured-error handler (see [`LogAction`]).
///
/// Split out of the handler closure in [`create_device`] so the discipline —
/// log the first few, announce the latch once, then go quiet except an
/// occasional debug bump — is unit-testable without a GPU or a real
/// `wgpu::Error`.
fn decide_log_action(count: u32) -> LogAction {
    if count <= MAX_LOGGED_UNCAPTURED_ERRORS {
        LogAction::Log
    } else if count == MAX_LOGGED_UNCAPTURED_ERRORS + 1 {
        LogAction::SuppressionNotice
    } else {
        LogAction::Silent {
            debug_bump: count.is_multiple_of(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::DownlevelProfile;

    #[test]
    fn emulator_strips_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, true);
        assert!(!flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(!flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn physical_device_keeps_debug_and_validation() {
        let build_flags = wgpu::InstanceFlags::DEBUG | wgpu::InstanceFlags::VALIDATION;
        let flags = effective_instance_flags(build_flags, false);
        assert!(flags.contains(wgpu::InstanceFlags::DEBUG));
        assert!(flags.contains(wgpu::InstanceFlags::VALIDATION));
    }

    #[test]
    fn emulator_with_no_debug_flags_stays_empty() {
        let flags = effective_instance_flags(wgpu::InstanceFlags::empty(), true);
        assert!(flags.is_empty());
    }

    #[test]
    fn ios_simulator_bumps_alignment_to_256() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        // Nothing else about the base limits should change.
        assert_eq!(
            wgpu::Limits {
                min_uniform_buffer_offset_alignment: base.min_uniform_buffer_offset_alignment,
                ..limits.clone()
            },
            base
        );
    }

    #[test]
    fn non_simulator_leaves_limits_untouched() {
        let base = wgpu::Limits::default();
        let limits = effective_limits(base.clone(), false);
        assert_eq!(limits, base);
    }

    #[test]
    fn base_already_at_or_above_256_is_not_lowered() {
        let base = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 512,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(base.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 512);
        assert_eq!(limits, base);
    }

    #[test]
    fn simulator_alignment_uses_real_adapter_limits_not_defaults() {
        // A low-alignment adapter (the #7057 shape: Metal reports a lower
        // alignment than the simulator driver actually enforces) is bumped to
        // 256 while every other adapter-reported limit is preserved — the
        // whole point of feeding *adapter* limits rather than `Limits::default`.
        let adapter = wgpu::Limits {
            min_uniform_buffer_offset_alignment: 64,
            max_texture_dimension_2d: 4096,
            ..wgpu::Limits::default()
        };
        let limits = effective_limits(adapter.clone(), true);
        assert_eq!(limits.min_uniform_buffer_offset_alignment, 256);
        assert_eq!(limits.max_texture_dimension_2d, 4096);
    }

    #[test]
    fn first_n_uncaptured_errors_log() {
        for count in 1..=MAX_LOGGED_UNCAPTURED_ERRORS {
            assert_eq!(
                decide_log_action(count),
                LogAction::Log,
                "expected Log at count={count}"
            );
        }
    }

    #[test]
    fn nplus1_uncaptured_error_suppresses() {
        assert_eq!(
            decide_log_action(MAX_LOGGED_UNCAPTURED_ERRORS + 1),
            LogAction::SuppressionNotice
        );
    }

    #[test]
    fn further_uncaptured_errors_stay_silent_between_debug_bumps() {
        let past_notice = MAX_LOGGED_UNCAPTURED_ERRORS + 2;
        assert_eq!(
            decide_log_action(past_notice),
            LogAction::Silent { debug_bump: false }
        );
    }

    #[test]
    fn uncaptured_error_debug_bump_is_periodic() {
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD * 2),
            LogAction::Silent { debug_bump: true }
        );
        assert_eq!(
            decide_log_action(UNCAPTURED_ERROR_DEBUG_BUMP_PERIOD + 1),
            LogAction::Silent { debug_bump: false }
        );
    }

    #[test]
    fn default_build_requires_no_device_features() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert!(caps.has_timestamp_query, "fixture precondition");
        assert_eq!(required_features(&caps, false), wgpu::Features::empty());
    }

    #[test]
    fn perf_trace_build_requires_timestamp_query_when_offered() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert_eq!(
            required_features(&caps, true),
            wgpu::Features::TIMESTAMP_QUERY
        );
    }

    #[test]
    fn perf_trace_build_requires_nothing_when_adapter_lacks_timestamp_query() {
        // An adapter that cannot do timestamp queries must still yield a
        // device: a required feature it lacks would fail the request outright,
        // so `perf-trace` degrades to no probes rather than to no GPU.
        let caps = TierCaps::fake(DownlevelProfile::WebGl2);
        assert!(!caps.has_timestamp_query, "fixture precondition");
        assert_eq!(required_features(&caps, true), wgpu::Features::empty());
    }

    #[test]
    fn default_options_label_the_device_and_leave_backends_to_the_environment() {
        let options = ContextOptions::default();
        assert_eq!(options.device_label, DEFAULT_DEVICE_LABEL);
        assert_eq!(options.backends, None);
    }

    #[test]
    fn a_fresh_context_has_no_device_and_therefore_no_caps() {
        // Construction must not enumerate adapters, so this is a host test, not
        // a GPU one: it passes on a machine with no usable GPU at all.
        let context = Context::new(ContextOptions::default());
        assert!(context.caps().is_none());
    }

    #[test]
    #[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu -- --ignored` \
                (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
    fn gpu_device_creation_reports_adapter_caps() {
        pollster::block_on(async {
            let mut context = Context::new(ContextOptions::default());
            let handle = context.device().await.expect("device creation");

            let info = handle.adapter.get_info();
            println!(
                "frust-gpu adapter: name={:?} backend={:?} device_type={:?} \
                 driver={:?} driver_info={:?} vendor={:#06x} device={:#06x}",
                info.name,
                info.backend,
                info.device_type,
                info.driver,
                info.driver_info,
                info.vendor,
                info.device
            );
            println!("frust-gpu caps: {:#?}", handle.caps);
            println!(
                "frust-gpu device limits: max_texture_dimension_2d={} \
                 min_uniform_buffer_offset_alignment={}",
                handle.device.limits().max_texture_dimension_2d,
                handle.device.limits().min_uniform_buffer_offset_alignment
            );

            assert_eq!(handle.caps.adapter_name, info.name);
            assert_eq!(handle.caps.backend, info.backend);
            assert!(!handle.caps.adapter_name.is_empty());
            assert!(handle.caps.max_texture_dimension_2d > 0);
            assert!(handle.caps.resource_texture_dim > 0);
            // Nothing has been submitted, so the latch must still be empty.
            assert_eq!(handle.first_uncaptured_error(), None);

            // With `FRUST_ENGINE_DOWNLEVEL=1` set for the whole test process
            // (the override is read once and cached in a `OnceLock`), both the
            // probed caps and the created device must actually report the
            // clamped WebGL2 shape rather than a desktop backend merely
            // relabelled `WebGl2`. Without it, this rig's real backend
            // (Vulkan/Metal/Dx12) must report `Full` unchanged, proving the
            // knob rehearses the downlevel shape rather than always forcing
            // it.
            let downlevel_env_set = std::env::var("FRUST_ENGINE_DOWNLEVEL").is_ok_and(|v| v != "0");
            if downlevel_env_set {
                assert_eq!(handle.caps.downlevel_profile, DownlevelProfile::WebGl2);
                assert!(!handle.caps.has_storage_buffers);
                assert!(handle.caps.max_texture_dimension_2d <= 2048);
                assert_eq!(handle.caps.min_uniform_buffer_offset_alignment, 256);
                assert!(handle.device.limits().max_texture_dimension_2d > 0);
            } else {
                assert_eq!(handle.caps.downlevel_profile, DownlevelProfile::Full);
            }

            let caps = context.caps().cloned().expect("caps after device creation");
            assert_eq!(caps.adapter_name, info.name);

            // The device is created once and reused: a second call must hand
            // back the same logical device, not build another one.
            let again = context.device().await.expect("device reuse");
            assert_eq!(again.caps, caps);
        });
    }
}
