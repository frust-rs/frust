//! The channels feature's use cases (huddle clean-architecture refactor)
//! — moved from the former flat `features/channels/mod.rs`, now behind
//! the [`ChannelRepository`](super::repositories::ChannelRepository) seam.
//!
//! - [`LoadChannels`] — the async roster loader (takes the repo `Arc`, the
//!   team-demo `LoadTeam` shape).
//! - [`ArchiveChannel`]/[`MuteChannel`] — synchronous flag echoes (the
//!   `SetTheme` shape): they read nothing, so they take no repository (an
//!   infallible read needs no repository ceremony).

pub mod archive_channel;
pub mod load_channels;
pub mod mute_channel;

pub use archive_channel::ArchiveChannel;
pub use load_channels::LoadChannels;
pub use mute_channel::MuteChannel;
