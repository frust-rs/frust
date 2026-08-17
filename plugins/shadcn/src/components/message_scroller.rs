//! Ports shadcn/ui's **MessageScroller** (the auto-scrolling chat message
//! viewport) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/message-scroller.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`).
//!
//! The upstream component wraps an external scroll-manager library
//! (`use-stick-to-bottom`) for its stick-to-bottom/scroll-to-latest
//! behavior; the port re-implements that manager on top of frust's own
//! `scroll_view` (this crate's `scroll_area` module wraps the same
//! baseline) rather than taking on the external dependency.
//!
//! Empty: the module exists so this crate's flat re-export surface is fixed
//! before the widget itself lands (see this directory's `mod.rs` for the
//! prefixing convention that keeps that flat surface collision-free).
