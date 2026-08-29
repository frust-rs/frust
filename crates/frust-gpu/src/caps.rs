//! Adapter capability probing: [`TierCaps`] and [`DownlevelProfile`].
//!
//! [`TierCaps`] is plain data pulled off a real `wgpu::Adapter` at device-init
//! time ([`TierCaps::probe`]), so downstream tier/pipeline decisions are
//! unit-testable against [`TierCaps::fake`] with no GPU in the loop — the same
//! pure-decision/platform-lookup split `frust-render`'s tier selection follows.
//!
//! [`DownlevelProfile`] tells a caller whether the adapter's limits are the
//! full desktop set or clamped to the GLES-3.0/WebGL2 downlevel defaults,
//! which a browser target (a future host) will always hit and which
//! `FRUST_ENGINE_DOWNLEVEL=1` lets a desktop developer rehearse today.

use std::sync::OnceLock;

/// The GLES-3.0/WebGL2 downlevel default resource-texture ceiling
/// (`wgpu::Limits::downlevel_webgl2_defaults().max_texture_dimension_2d`).
const WEBGL2_MAX_TEXTURE_DIMENSION_2D: u32 = 2048;

/// The desktop (Vulkan/Metal/Dx12) default resource-texture ceiling
/// (`wgpu::Limits::defaults().max_texture_dimension_2d`).
const DESKTOP_MAX_TEXTURE_DIMENSION_2D: u32 = 8192;

/// The ceiling [`TierCaps::resource_texture_dim`] never exceeds regardless of
/// what the adapter itself reports, keeping any texture-pool/atlas sizing
/// decision built on it bounded even on a very generous desktop adapter.
const MAX_RESOURCE_TEXTURE_DIM: u32 = 4096;

/// The texture format a texture atlas is backed by. `Rgba8Unorm` is the one
/// target format used everywhere else a `wgpu::TextureFormat` is chosen in
/// this workspace's GPU backend.
const ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Whether an adapter's usable limits are the full native set, or clamped to
/// the GLES-3.0/WebGL2 downlevel defaults (`wgpu::Limits::downlevel_webgl2_defaults`).
///
/// Derived, never chosen directly by a caller building [`TierCaps::probe`]:
/// `WebGl2` whenever the adapter's backend is [`wgpu::Backend::Gl`], or
/// whenever the process-wide `FRUST_ENGINE_DOWNLEVEL=1` override is set — the
/// latter lets a desktop (Vulkan/Metal/Dx12) adapter rehearse the browser's
/// downlevel ceiling without an actual GL/WebGL2 context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownlevelProfile {
    /// Native desktop/mobile limits (Vulkan, Metal, Dx12).
    Full,
    /// Clamped to the GLES-3.0/WebGL2 downlevel default limits.
    WebGl2,
}

impl DownlevelProfile {
    fn resolve(backend: wgpu::Backend) -> Self {
        if backend == wgpu::Backend::Gl || downlevel_override_enabled() {
            DownlevelProfile::WebGl2
        } else {
            DownlevelProfile::Full
        }
    }
}

/// Whether the `FRUST_ENGINE_DOWNLEVEL` process-wide override is set to a
/// non-zero value, checking both the compile-time (`option_env!`) and runtime
/// (`std::env::var`) halves like `frust-render`'s `FRUST_TRACE`/
/// `FRUST_NO_DIRECT_SURFACE` knobs. Cached: read once per process.
fn downlevel_override_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        env_flag_enabled(
            option_env!("FRUST_ENGINE_DOWNLEVEL"),
            std::env::var("FRUST_ENGINE_DOWNLEVEL").ok(),
        )
    })
}

fn env_flag_enabled(compile_time: Option<&str>, runtime: Option<String>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime.as_deref())
}

/// Plain-data adapter capabilities the GPU crate's device/pipeline/atlas
/// decisions are built over. Constructed from a real `wgpu::Adapter` via
/// [`TierCaps::probe`], or by hand in host tests via [`TierCaps::fake`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierCaps {
    /// The adapter's `wgpu::DownlevelCapabilities::flags`.
    pub downlevel_flags: wgpu::DownlevelFlags,
    /// The adapter's `wgpu::AdapterInfo::name`, used only for diagnostics.
    pub adapter_name: String,
    /// The adapter's `wgpu::AdapterInfo::backend`.
    pub backend: wgpu::Backend,
    /// The adapter's `wgpu::Limits::max_texture_dimension_2d`.
    pub max_texture_dimension_2d: u32,
    /// The adapter's `wgpu::Limits::max_texture_array_layers`.
    pub max_texture_array_layers: u32,
    /// The adapter's `wgpu::Limits::max_bind_groups`.
    pub max_bind_groups: u32,
    /// The adapter's `wgpu::Limits::max_uniform_buffer_binding_size`,
    /// saturated into a `u32` (the field is `u64` on `wgpu::Limits`; no
    /// adapter this workspace targets reports a value anywhere near
    /// `u32::MAX`).
    pub max_uniform_buffer_binding_size: u32,
    /// The adapter's `wgpu::Limits::min_uniform_buffer_offset_alignment`.
    pub min_uniform_buffer_offset_alignment: u32,
    /// The adapter's `wgpu::Limits::max_vertex_attributes`.
    pub max_vertex_attributes: u32,
    /// Whether the adapter supports storage buffers at all
    /// (`wgpu::Limits::max_storage_buffers_per_shader_stage > 0`) — `false`
    /// under the GLES-3.0/WebGL2 downlevel default limits.
    pub has_storage_buffers: bool,
    /// Whether the adapter exposes `wgpu::Features::TIMESTAMP_QUERY`.
    pub has_timestamp_query: bool,
    /// The adapter's `wgpu::AdapterInfo::transient_saves_memory` — whether
    /// adding `wgpu::TextureUsages::TRANSIENT` to a texture (which itself
    /// requires `wgpu::StoreOp::Discard`) reduces memory usage on this
    /// adapter.
    pub transient_saves_memory: bool,
    /// The texture format a texture atlas is backed by.
    pub atlas_format: wgpu::TextureFormat,
    /// The texture dimension a pooled/atlas resource texture is sized
    /// against: `min(max_texture_dimension_2d, 4096)`, so a very generous
    /// desktop adapter's real ceiling never drives an oversized allocation.
    pub resource_texture_dim: u32,
    /// Whether this adapter's usable limits are the full native set or
    /// clamped to the GLES-3.0/WebGL2 downlevel defaults.
    pub downlevel_profile: DownlevelProfile,
}

impl TierCaps {
    /// Probes a real `wgpu::Adapter` for the capabilities this crate's
    /// device/pipeline/atlas decisions are built over. Synchronous: every
    /// value it reads (`get_info`, `get_downlevel_capabilities`, `limits`,
    /// `features`) is available before device creation.
    pub fn probe(adapter: &wgpu::Adapter) -> Self {
        let info = adapter.get_info();
        let downlevel = adapter.get_downlevel_capabilities();
        let limits = adapter.limits();
        let features = adapter.features();
        let downlevel_profile = DownlevelProfile::resolve(info.backend);
        let max_texture_dimension_2d = limits.max_texture_dimension_2d;
        Self {
            downlevel_flags: downlevel.flags,
            adapter_name: info.name,
            backend: info.backend,
            max_texture_dimension_2d,
            max_texture_array_layers: limits.max_texture_array_layers,
            max_bind_groups: limits.max_bind_groups,
            max_uniform_buffer_binding_size: saturating_u32(limits.max_uniform_buffer_binding_size),
            min_uniform_buffer_offset_alignment: limits.min_uniform_buffer_offset_alignment,
            max_vertex_attributes: limits.max_vertex_attributes,
            has_storage_buffers: limits.max_storage_buffers_per_shader_stage > 0,
            has_timestamp_query: features.contains(wgpu::Features::TIMESTAMP_QUERY),
            transient_saves_memory: info.transient_saves_memory,
            atlas_format: ATLAS_FORMAT,
            resource_texture_dim: max_texture_dimension_2d.min(MAX_RESOURCE_TEXTURE_DIM),
            downlevel_profile,
        }
    }

    /// Builds synthetic caps for the given [`DownlevelProfile`] with no
    /// `wgpu::Adapter` at all — the host-test counterpart to [`Self::probe`].
    ///
    /// `Full` mirrors `wgpu::Limits::defaults()`'s desktop values (backend
    /// `Vulkan`, `max_texture_dimension_2d` 8192, storage buffers present);
    /// `WebGl2` mirrors `wgpu::Limits::downlevel_webgl2_defaults()` (backend
    /// `Gl`, `max_texture_dimension_2d` 2048, no storage buffers).
    pub fn fake(profile: DownlevelProfile) -> Self {
        match profile {
            DownlevelProfile::Full => Self {
                downlevel_flags: wgpu::DownlevelFlags::all(),
                adapter_name: "fake-full".to_string(),
                backend: wgpu::Backend::Vulkan,
                max_texture_dimension_2d: DESKTOP_MAX_TEXTURE_DIMENSION_2D,
                max_texture_array_layers: 256,
                max_bind_groups: 4,
                max_uniform_buffer_binding_size: 64 << 10,
                min_uniform_buffer_offset_alignment: 256,
                max_vertex_attributes: 16,
                has_storage_buffers: true,
                has_timestamp_query: true,
                transient_saves_memory: false,
                atlas_format: ATLAS_FORMAT,
                resource_texture_dim: DESKTOP_MAX_TEXTURE_DIMENSION_2D
                    .min(MAX_RESOURCE_TEXTURE_DIM),
                downlevel_profile: DownlevelProfile::Full,
            },
            DownlevelProfile::WebGl2 => Self {
                downlevel_flags: wgpu::DownlevelFlags::empty(),
                adapter_name: "fake-webgl2".to_string(),
                backend: wgpu::Backend::Gl,
                max_texture_dimension_2d: WEBGL2_MAX_TEXTURE_DIMENSION_2D,
                max_texture_array_layers: 256,
                max_bind_groups: 4,
                max_uniform_buffer_binding_size: 16 << 10,
                min_uniform_buffer_offset_alignment: 256,
                max_vertex_attributes: 16,
                has_storage_buffers: false,
                has_timestamp_query: false,
                transient_saves_memory: false,
                atlas_format: ATLAS_FORMAT,
                resource_texture_dim: WEBGL2_MAX_TEXTURE_DIMENSION_2D.min(MAX_RESOURCE_TEXTURE_DIM),
                downlevel_profile: DownlevelProfile::WebGl2,
            },
        }
    }
}

/// `u64 -> u32` saturating conversion for `wgpu::Limits` fields declared
/// wider than the `u32` this crate's [`TierCaps`] stores them as.
fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_profile_fake_reports_desktop_defaults() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert_eq!(caps.downlevel_profile, DownlevelProfile::Full);
        assert_eq!(caps.backend, wgpu::Backend::Vulkan);
        assert!(caps.has_storage_buffers);
        assert_eq!(
            caps.max_texture_dimension_2d,
            DESKTOP_MAX_TEXTURE_DIMENSION_2D
        );
        assert_eq!(caps.resource_texture_dim, MAX_RESOURCE_TEXTURE_DIM);
    }

    #[test]
    fn webgl2_profile_fake_reports_clamped_resource_dim_and_no_storage_buffers() {
        let caps = TierCaps::fake(DownlevelProfile::WebGl2);
        assert_eq!(caps.downlevel_profile, DownlevelProfile::WebGl2);
        assert!(caps.resource_texture_dim <= 2048);
        assert!(!caps.has_storage_buffers);
    }

    #[test]
    fn resource_texture_dim_never_exceeds_the_shared_ceiling() {
        let caps = TierCaps::fake(DownlevelProfile::Full);
        assert!(caps.resource_texture_dim <= MAX_RESOURCE_TEXTURE_DIM);
    }

    #[test]
    fn atlas_format_is_set_on_both_profiles() {
        assert_eq!(
            TierCaps::fake(DownlevelProfile::Full).atlas_format,
            ATLAS_FORMAT
        );
        assert_eq!(
            TierCaps::fake(DownlevelProfile::WebGl2).atlas_format,
            ATLAS_FORMAT
        );
    }

    #[test]
    fn saturating_u32_clamps_a_too_large_u64() {
        assert_eq!(saturating_u32(u64::MAX), u32::MAX);
        assert_eq!(saturating_u32(64 << 10), 64 << 10);
    }
}
