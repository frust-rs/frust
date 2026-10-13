//! Slivers: the children a [`CustomScrollView`](crate::CustomScrollView) lays
//! out with the sliver protocol ([`frust_core::SliverConstraints`] in,
//! [`frust_core::SliverGeometry`] out) instead of box constraints.
//!
//! Each sliver lives in its own submodule and is re-exported flat from here
//! and from the crate root.

pub mod adapter;

pub use adapter::{SliverBoxView, SliverBoxWidget, sliver_box};
