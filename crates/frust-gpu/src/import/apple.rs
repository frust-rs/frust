//! Apple zero-copy import: CVPixelBuffer -> CVMetalTextureCache -> MTLTexture ->
//! `wgpu_hal::metal::Device::texture_from_raw` -> `wgpu::Device::create_texture_from_hal`.
//! Seeded empty; the Apple import card fills it in.
