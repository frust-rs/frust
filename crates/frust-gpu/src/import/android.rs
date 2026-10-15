//! Android zero-copy import: AHardwareBuffer -> VkImage bound to external memory on a
//! Vulkan device opened with `VK_ANDROID_external_memory_android_hardware_buffer` ->
//! `wgpu_hal::vulkan::Device::texture_from_raw(.., TextureMemory::External)`.
//!
//! [`import_hardware_buffer`] is the whole surface: a buffer in, a sampleable
//! `wgpu::Texture` aliasing its memory out, with no copy. It works only on a
//! device [`crate::context`] opened itself with the import extensions — the
//! one whose [`TierCaps::hwbuffer_import`](crate::caps::TierCaps::hwbuffer_import)
//! is `true`; on any other device it answers
//! [`HardwareBufferImportError::Unsupported`].
//!
//! # What one import does
//!
//! 1. `AHardwareBuffer_describe` and `vkGetAndroidHardwareBufferPropertiesANDROID`
//!    — the buffer's extent, layout and allocation. Only a single-layer
//!    `R8G8B8A8_UNORM` buffer with `GPU_SAMPLED_IMAGE` usage, reporting
//!    `VK_FORMAT_R8G8B8A8_UNORM` (no external format) and sampleable, within
//!    the device's 2D texture limit, is imported; anything else is refused
//!    (`check_buffer`), so an NV12 or `R8G8B8X8` buffer stays on the CPU path.
//! 2. A 2D, single-mip, optimal-tiling `VkImage` with `SAMPLED` usage, created
//!    with `VkExternalMemoryImageCreateInfo` naming the
//!    `ANDROID_HARDWARE_BUFFER_ANDROID` handle type.
//! 3. A dedicated `VkDeviceMemory` importing the buffer
//!    (`VkImportAndroidHardwareBufferInfoANDROID` + `VkMemoryDedicatedAllocateInfo`),
//!    of the size and one of the memory types the buffer's properties name
//!    (device-local first), bound to the image at offset 0.
//! 4. One queue-family ownership acquire from `VK_QUEUE_FAMILY_FOREIGN_EXT`,
//!    `UNDEFINED -> SHADER_READ_ONLY_OPTIMAL`, recorded into a one-off command
//!    buffer, submitted on the device's queue and waited on — the transfer
//!    that makes the producer's writes visible and keeps the contents (a plain
//!    `UNDEFINED` transition may discard them).
//! 5. `texture_from_raw(.., TextureMemory::External)` with a drop callback, then
//!    `create_texture_from_hal` with `TextureUses::RESOURCE` — the layout step 4
//!    left the image in, so `wgpu`'s first barrier starts from the truth.
//!
//! # Invariants
//!
//! - **The buffer outlives the texture.** The import takes its own
//!   `AHardwareBuffer_acquire` reference (on top of the one the Vulkan memory
//!   import holds), so the caller may release its own as soon as the call
//!   returns — which is what the frame bus does.
//! - **One owner frees everything, once.** The image, its memory and that
//!   reference belong to the drop callback, which `wgpu` runs when the texture
//!   is destroyed — after every submission using it has finished, and while
//!   the device (which every `wgpu` resource keeps alive) still exists. `wgpu`
//!   destroys neither the image (a callback is present) nor the memory
//!   (`External`). Every early return before the hand-off destroys what it had
//!   created so far (`Partial`).
//! - **Queue access is externally synchronised.** Step 4 submits on the raw
//!   queue `wgpu` also submits and presents on, so the call must not overlap a
//!   `wgpu` submit or present on another thread. The frame bus calls importers
//!   inside the external-pass drain on the thread that records, submits and
//!   presents the frame, which satisfies it.
//! - **No link-time `AHardwareBuffer` symbol.** The three buffer entry points
//!   used here are resolved from `libandroid.so` at run time (`BufferApi`), not
//!   linked: they exist only from API 26, while Android libraries are linked
//!   against the API-21 stubs by default, where a direct reference fails the
//!   link. On a release without them the import answers `Unsupported`.
//! - **The texture is read-only.** It carries `TEXTURE_BINDING` only; nothing
//!   writes to it, and nothing hands ownership back to the foreign queue
//!   family, so a producer must not write the buffer again while a texture
//!   over it is alive.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt;
use std::sync::OnceLock;

use ash::vk;
use wgpu::hal::api::Vulkan;
use wgpu::hal::vulkan::TextureMemory;

/// How long the ownership acquire (step 4 in the module docs) may take before
/// the import gives up: one barrier on an idle-ish queue completes in well
/// under a millisecond, so a second means a wedged queue.
const ACQUIRE_TIMEOUT_NS: u64 = 1_000_000_000;

/// The one buffer layout imported today.
const BUFFER_FORMAT: u32 = ndk_sys::AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM.0;

/// The buffer usage bit a sampled import requires. The flag type is a C
/// `unsigned long` — 32 bits on the 32-bit ABIs — so the widening cast is
/// needed there and only looks redundant on 64-bit ones.
#[allow(clippy::unnecessary_cast)]
const BUFFER_USAGE_SAMPLED: u64 =
    ndk_sys::AHardwareBuffer_UsageFlags::AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE.0 as u64;

/// Why [`import_hardware_buffer`] could not import a buffer. Mirrors the
/// frame bus's own import error, which a shell's importer maps it onto.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HardwareBufferImportError {
    /// The device cannot import hardware buffers at all — not a Vulkan device,
    /// or one opened without the import extensions.
    Unsupported(String),
    /// This buffer could not be imported: an unsupported layout or a driver
    /// error.
    Failed(String),
}

impl fmt::Display for HardwareBufferImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "hardware-buffer import unsupported: {reason}"),
            Self::Failed(reason) => write!(f, "hardware-buffer import failed: {reason}"),
        }
    }
}

impl std::error::Error for HardwareBufferImportError {}

/// The `AHardwareBuffer` entry points an import calls, resolved once from
/// `libandroid.so` (see the module invariants for why they are not linked).
struct BufferApi {
    acquire: unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer),
    release: unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer),
    describe:
        unsafe extern "C" fn(*const ndk_sys::AHardwareBuffer, *mut ndk_sys::AHardwareBuffer_Desc),
}

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// Bionic's `RTLD_NOW`, which differs between the 32- and 64-bit ABIs.
#[cfg(target_pointer_width = "64")]
const RTLD_NOW: c_int = 2;
#[cfg(not(target_pointer_width = "64"))]
const RTLD_NOW: c_int = 0;

/// The process's [`BufferApi`], or `None` on a release older than API 26.
fn buffer_api() -> Option<&'static BufferApi> {
    static API: OnceLock<Option<BufferApi>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: `libandroid.so` is a public NDK library, already loaded in
        // every app process; the handle is never closed, so the resolved
        // addresses stay valid for the process. Each symbol is transmuted to
        // the signature the NDK headers declare for it.
        unsafe {
            let library = dlopen(c"libandroid.so".as_ptr(), RTLD_NOW);
            if library.is_null() {
                return None;
            }
            let symbol = |name: &CStr| {
                let address = dlsym(library, name.as_ptr());
                (!address.is_null()).then_some(address)
            };
            Some(BufferApi {
                acquire: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer),
                >(symbol(c"AHardwareBuffer_acquire")?),
                release: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer),
                >(symbol(c"AHardwareBuffer_release")?),
                describe: std::mem::transmute::<
                    *mut c_void,
                    unsafe extern "C" fn(
                        *const ndk_sys::AHardwareBuffer,
                        *mut ndk_sys::AHardwareBuffer_Desc,
                    ),
                >(symbol(c"AHardwareBuffer_describe")?),
            })
        }
    })
    .as_ref()
}

fn failed(step: &str, result: vk::Result) -> HardwareBufferImportError {
    HardwareBufferImportError::Failed(format!("{step}: {result}"))
}

/// Wraps `buffer` — an `AHardwareBuffer` in the `R8G8B8A8_UNORM` layout — as a
/// sampleable `Rgba8Unorm` `wgpu::Texture` on `device`, without a copy.
///
/// The texture has `TEXTURE_BINDING` usage only, one mip level, and the
/// buffer's extent; its texels are the buffer's bytes as the producer left
/// them (the frame bus samples them as premultiplied RGBA). The texture keeps
/// its own reference to the buffer; the caller keeps ownership of the
/// reference it passed in.
///
/// # Errors
///
/// [`HardwareBufferImportError::Unsupported`] on a device opened without the
/// import extensions ([`TierCaps::hwbuffer_import`](crate::caps::TierCaps::hwbuffer_import)
/// `false`); [`HardwareBufferImportError::Failed`] for a null pointer, a buffer
/// whose layout is not imported (see the module docs), or a Vulkan error.
///
/// # Safety
///
/// - `buffer` is null or a valid `AHardwareBuffer*` the caller holds a
///   reference to for the duration of the call.
/// - No other thread submits to or presents on `device`'s queue during the
///   call (the module's queue invariant).
/// - The buffer's contents are complete — any producer fence has signalled —
///   and the producer does not write it again while the returned texture is
///   alive.
pub unsafe fn import_hardware_buffer(
    device: &wgpu::Device,
    buffer: *mut ndk_sys::AHardwareBuffer,
) -> Result<wgpu::Texture, HardwareBufferImportError> {
    if buffer.is_null() {
        return Err(HardwareBufferImportError::Failed(
            "a null AHardwareBuffer".to_string(),
        ));
    }

    let api = buffer_api().ok_or_else(|| {
        HardwareBufferImportError::Unsupported(
            "this Android release has no AHardwareBuffer API (API 26+)".to_string(),
        )
    })?;
    // SAFETY: `buffer` is a live buffer the caller holds (this fn's contract);
    // `describe` only writes the plain-data descriptor.
    let description = unsafe {
        let mut description = std::mem::zeroed::<ndk_sys::AHardwareBuffer_Desc>();
        (api.describe)(buffer, &mut description);
        description
    };
    let max_dimension = device.limits().max_texture_dimension_2d;
    let (width, height) = check_buffer(&description, max_dimension)?;

    // SAFETY: the guard only borrows the device's hal object for the import;
    // nothing here destroys it, and every raw object created below is either
    // destroyed before returning or handed to `wgpu` with the texture.
    let hal_device = unsafe { device.as_hal::<Vulkan>() }.ok_or_else(|| {
        HardwareBufferImportError::Unsupported("the device is not a Vulkan device".to_string())
    })?;
    let enabled = hal_device.enabled_device_extensions();
    for required in [
        ash::android::external_memory_android_hardware_buffer::NAME,
        ash::ext::queue_family_foreign::NAME,
    ] {
        if !enabled.contains(&required) {
            return Err(HardwareBufferImportError::Unsupported(format!(
                "the device was opened without {required:?}"
            )));
        }
    }
    let raw = hal_device.raw_device().clone();
    let instance = hal_device.shared_instance().raw_instance();
    let extension =
        ash::android::external_memory_android_hardware_buffer::Device::new(instance, &raw);

    // Step 1: the buffer's allocation and Vulkan-side layout.
    let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
    let mut properties =
        vk::AndroidHardwareBufferPropertiesANDROID::default().push_next(&mut format_properties);
    // SAFETY: the import extension is enabled on `raw` (checked above) and
    // `buffer` is live.
    unsafe {
        extension.get_android_hardware_buffer_properties(buffer.cast::<c_void>(), &mut properties)
    }
    .map_err(|e| failed("vkGetAndroidHardwareBufferPropertiesANDROID", e))?;
    let allocation_size = properties.allocation_size;
    let memory_type_bits = properties.memory_type_bits;
    if format_properties.format != vk::Format::R8G8B8A8_UNORM
        || !format_properties
            .format_features
            .contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
    {
        return Err(HardwareBufferImportError::Failed(format!(
            "the buffer maps to {:?} with {:?}, not a sampleable R8G8B8A8_UNORM",
            format_properties.format, format_properties.format_features
        )));
    }
    // SAFETY: a plain query on the physical device this device was opened on.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(hal_device.raw_physical_device()) };
    let memory_type = pick_memory_type(memory_type_bits, &memory_properties).ok_or_else(|| {
        HardwareBufferImportError::Failed(format!(
            "no memory type in the buffer's mask {memory_type_bits:#x}"
        ))
    })?;

    let mut partial = Partial {
        device: raw.clone(),
        image: vk::Image::null(),
        memory: vk::DeviceMemory::null(),
    };

    // Step 2: the image the memory backs.
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external);
    // SAFETY: a valid create info on a live device; `partial` owns the result.
    partial.image =
        unsafe { raw.create_image(&image_info, None) }.map_err(|e| failed("vkCreateImage", e))?;

    // Step 3: the buffer's memory, imported and bound.
    let mut import = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(buffer.cast());
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(partial.image);
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(allocation_size)
        .memory_type_index(memory_type)
        .push_next(&mut import)
        .push_next(&mut dedicated);
    // SAFETY: size and type come from the buffer's own properties, the
    // dedicated image is the one just created; `partial` owns the result.
    partial.memory = unsafe { raw.allocate_memory(&allocate_info, None) }
        .map_err(|e| failed("vkAllocateMemory (AHardwareBuffer import)", e))?;
    // SAFETY: a dedicated allocation for exactly this image, bound once.
    unsafe { raw.bind_image_memory(partial.image, partial.memory, 0) }
        .map_err(|e| failed("vkBindImageMemory", e))?;

    // Step 4: take the buffer over from the foreign producer.
    // SAFETY: the queue invariant is this fn's caller contract; the family is
    // the one the queue was created from.
    unsafe {
        acquire_from_foreign(
            &raw,
            hal_device.raw_queue(),
            hal_device.queue_family_index(),
            partial.image,
        )
    }?;

    // Step 5: hand everything to the texture.
    let (image, memory) = partial.disarm();
    // SAFETY: `buffer` is live; this reference is the texture's, released by
    // the drop callback below.
    unsafe { (api.acquire)(buffer) };
    let held = HeldBuffer {
        buffer,
        release: api.release,
    };
    let drop_device = raw;
    let on_drop: wgpu::hal::DropCallback = Box::new(move || {
        // SAFETY: `wgpu` runs this once, after the last submission using the
        // texture finished and while the device is alive; nothing else owns
        // these objects (the module invariants).
        unsafe {
            drop_device.destroy_image(image, None);
            drop_device.free_memory(memory, None);
        }
        held.release();
    });

    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let hal_descriptor = wgpu::hal::TextureDescriptor {
        label: Some("frust-gpu hardware buffer"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUses::RESOURCE,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    };
    // SAFETY: the image was created for exactly `hal_descriptor` (2D, one mip,
    // one sample, RGBA8, no mutable format and no view formats); the memory is
    // external and lives until `on_drop` runs.
    let hal_texture = unsafe {
        hal_device.texture_from_raw(
            image,
            &hal_descriptor,
            Some(on_drop),
            TextureMemory::External,
        )
    };
    drop(hal_device);

    let descriptor = wgpu::TextureDescriptor {
        label: Some("frust-gpu hardware buffer"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    // SAFETY: the hal texture came from this device and matches `descriptor`;
    // its contents are the buffer's (initialised), and step 4 left it in the
    // `SHADER_READ_ONLY_OPTIMAL` layout `RESOURCE` names.
    Ok(unsafe {
        device.create_texture_from_hal::<Vulkan>(
            hal_texture,
            &descriptor,
            wgpu::TextureUses::RESOURCE,
        )
    })
}

/// The extent of a buffer [`import_hardware_buffer`] imports, or why it is
/// refused: single-layer `R8G8B8A8_UNORM` with `GPU_SAMPLED_IMAGE` usage, a
/// non-empty extent within `max_dimension`.
fn check_buffer(
    description: &ndk_sys::AHardwareBuffer_Desc,
    max_dimension: u32,
) -> Result<(u32, u32), HardwareBufferImportError> {
    let refuse = |why: String| Err(HardwareBufferImportError::Failed(why));
    if description.format != BUFFER_FORMAT {
        return refuse(format!(
            "AHardwareBuffer format {} is not R8G8B8A8_UNORM",
            description.format
        ));
    }
    if description.layers != 1 {
        return refuse(format!("{} layers, not 1", description.layers));
    }
    if description.usage & BUFFER_USAGE_SAMPLED == 0 {
        return refuse("the buffer lacks GPU_SAMPLED_IMAGE usage".to_string());
    }
    let (width, height) = (description.width, description.height);
    if width == 0 || height == 0 || width > max_dimension || height > max_dimension {
        return refuse(format!(
            "a {width}x{height} buffer is outside 1..={max_dimension}"
        ));
    }
    Ok((width, height))
}

/// The memory type an import allocates from: the first one in `type_bits`
/// that is device-local, else the first one in `type_bits` at all.
fn pick_memory_type(
    type_bits: u32,
    properties: &vk::PhysicalDeviceMemoryProperties,
) -> Option<u32> {
    let candidates = || {
        properties
            .memory_types_as_slice()
            .iter()
            .enumerate()
            .filter(move |(index, _)| type_bits & (1 << index) != 0)
    };
    candidates()
        .find(|(_, memory)| {
            memory
                .property_flags
                .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
        })
        .or_else(|| candidates().next())
        .map(|(index, _)| index as u32)
}

/// Records, submits and waits for one queue-family ownership acquire of
/// `image` from `VK_QUEUE_FAMILY_FOREIGN_EXT` into `family`, transitioning it
/// `UNDEFINED -> SHADER_READ_ONLY_OPTIMAL`. Every object it creates is
/// destroyed before it returns.
///
/// # Safety
///
/// `device`, `queue` and `image` are live, `queue` belongs to `family`, and no
/// other thread uses `queue` during the call.
unsafe fn acquire_from_foreign(
    device: &ash::Device,
    queue: vk::Queue,
    family: u32,
    image: vk::Image,
) -> Result<(), HardwareBufferImportError> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::TRANSIENT)
        .queue_family_index(family);
    // SAFETY: (all blocks in this fn) the caller's contract; the pool and the
    // fence are destroyed on every path below.
    let pool = unsafe { device.create_command_pool(&pool_info, None) }
        .map_err(|e| failed("vkCreateCommandPool", e))?;
    let result = unsafe { record_and_submit(device, queue, family, pool, image) };
    unsafe { device.destroy_command_pool(pool, None) };
    result
}

/// [`acquire_from_foreign`]'s body, inside the pool it owns.
///
/// # Safety
///
/// As [`acquire_from_foreign`], with `pool` created for `family`.
unsafe fn record_and_submit(
    device: &ash::Device,
    queue: vk::Queue,
    family: u32,
    pool: vk::CommandPool,
    image: vk::Image,
) -> Result<(), HardwareBufferImportError> {
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: (all blocks in this fn) the caller's contract; the command
    // buffer is freed with its pool by the caller.
    let command_buffer = unsafe { device.allocate_command_buffers(&allocate_info) }
        .map_err(|e| failed("vkAllocateCommandBuffers", e))?[0];
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(|e| failed("vkBeginCommandBuffer", e))?;
    let barrier = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_FOREIGN_EXT)
        .dst_queue_family_index(family)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
    unsafe { device.end_command_buffer(command_buffer) }
        .map_err(|e| failed("vkEndCommandBuffer", e))?;

    let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
        .map_err(|e| failed("vkCreateFence", e))?;
    let command_buffers = [command_buffer];
    let submit = vk::SubmitInfo::default().command_buffers(&command_buffers);
    let result = unsafe { device.queue_submit(queue, &[submit], fence) }
        .map_err(|e| failed("vkQueueSubmit (ownership acquire)", e))
        .and_then(|()| {
            unsafe { device.wait_for_fences(&[fence], true, ACQUIRE_TIMEOUT_NS) }
                .map_err(|e| failed("vkWaitForFences (ownership acquire)", e))
        });
    if result.is_err() {
        // A submitted command buffer must not be freed while pending: wait it
        // out before the caller destroys the pool. Best effort — a lost
        // device has nothing pending.
        let _ = unsafe { device.queue_wait_idle(queue) };
    }
    unsafe { device.destroy_fence(fence, None) };
    result
}

/// The image and memory of an import still in progress, destroyed on drop
/// unless [`Self::disarm`] hands them on.
struct Partial {
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
}

impl Partial {
    /// Hands the image and memory to the caller; dropping `self` then frees
    /// nothing.
    fn disarm(mut self) -> (vk::Image, vk::DeviceMemory) {
        let parts = (self.image, self.memory);
        self.image = vk::Image::null();
        self.memory = vk::DeviceMemory::null();
        parts
    }
}

impl Drop for Partial {
    fn drop(&mut self) {
        // SAFETY: both were created on `device` by this import, never
        // submitted (an import that fails before or during the acquire has
        // waited its submission out), and null handles are ignored by Vulkan.
        unsafe {
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// The texture's own reference to its buffer, released by the texture's drop
/// callback.
struct HeldBuffer {
    buffer: *mut ndk_sys::AHardwareBuffer,
    release: unsafe extern "C" fn(*mut ndk_sys::AHardwareBuffer),
}

// SAFETY: an `AHardwareBuffer` is reference-counted and may be acquired and
// released from any thread; this wrapper only ever releases its one reference.
unsafe impl Send for HeldBuffer {}
// SAFETY: as `Send` — the wrapper exposes nothing through a shared reference.
unsafe impl Sync for HeldBuffer {}

impl HeldBuffer {
    fn release(self) {
        // SAFETY: the reference `import_hardware_buffer` acquired for the
        // texture, released exactly once (this consumes the wrapper).
        unsafe { (self.release)(self.buffer) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(width: u32, height: u32) -> ndk_sys::AHardwareBuffer_Desc {
        ndk_sys::AHardwareBuffer_Desc {
            width,
            height,
            layers: 1,
            format: BUFFER_FORMAT,
            usage: BUFFER_USAGE_SAMPLED,
            stride: width,
            rfu0: 0,
            rfu1: 0,
        }
    }

    #[test]
    fn a_sampled_rgba8_buffer_within_the_limit_is_imported() {
        assert_eq!(check_buffer(&description(64, 32), 4096), Ok((64, 32)));
    }

    #[test]
    fn other_layouts_are_refused() {
        let rgbx = ndk_sys::AHardwareBuffer_Desc {
            format: ndk_sys::AHardwareBuffer_Format::AHARDWAREBUFFER_FORMAT_R8G8B8X8_UNORM.0,
            ..description(64, 64)
        };
        assert!(check_buffer(&rgbx, 4096).is_err());
        let layered = ndk_sys::AHardwareBuffer_Desc {
            layers: 2,
            ..description(64, 64)
        };
        assert!(check_buffer(&layered, 4096).is_err());
        let unsampled = ndk_sys::AHardwareBuffer_Desc {
            usage: 0,
            ..description(64, 64)
        };
        assert!(check_buffer(&unsampled, 4096).is_err());
        assert!(check_buffer(&description(8192, 64), 4096).is_err());
        assert!(check_buffer(&description(0, 64), 4096).is_err());
    }

    #[test]
    fn device_local_memory_is_preferred_within_the_mask() {
        let mut properties = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: 3,
            ..Default::default()
        };
        properties.memory_types[0].property_flags = vk::MemoryPropertyFlags::DEVICE_LOCAL;
        properties.memory_types[1].property_flags = vk::MemoryPropertyFlags::HOST_VISIBLE;
        properties.memory_types[2].property_flags =
            vk::MemoryPropertyFlags::DEVICE_LOCAL | vk::MemoryPropertyFlags::HOST_VISIBLE;
        assert_eq!(pick_memory_type(0b110, &properties), Some(2));
        assert_eq!(pick_memory_type(0b010, &properties), Some(1));
        assert_eq!(pick_memory_type(0, &properties), None);
    }
}
