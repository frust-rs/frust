//! Android zero-copy import: AHardwareBuffer -> VkImage bound to external memory on a
//! Vulkan device opened with `VK_ANDROID_external_memory_android_hardware_buffer` ->
//! `wgpu_hal::vulkan::Device::texture_from_raw(.., TextureMemory::External)`.
//! Seeded empty; the Android import card fills it in.
