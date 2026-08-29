//! The `wgpu` adapter/device/surface substrate the render engine builds on.
//!
//! `frust-gpu` owns nothing rendering-specific — no scene display list, no
//! shader pipeline, no `vello`/`glifo` dependency. Its whole job is the layer
//! directly above `wgpu` itself: probing what an adapter can actually do
//! ([`caps::TierCaps`]) and, in later work, turning that into instances,
//! devices, surfaces and pooled GPU resources a renderer built on top of it
//! can consume without re-deriving adapter capabilities itself.
//!
//! [`caps::TierCaps::probe`] is the only place a real `wgpu::Adapter` is
//! consulted; every decision built on top of it takes the plain
//! [`caps::TierCaps`] value instead, so it stays testable with
//! [`caps::TierCaps::fake`] and no GPU in the loop.

pub mod caps;
pub mod context;

pub use caps::{DownlevelProfile, TierCaps};
pub use context::{Context, ContextOptions, DeviceHandle};
