//! Theme ladder L3's Apple half, as the iOS arm names it — a re-export of
//! [`crate::coretext`], where the implementation lives.
//!
//! The CoreText half never touched UIKit, so it moved to a top-level module
//! both Apple arms share (`crate::coretext`'s own doc has the why); this path
//! stays so every iOS call site (`crate::api::theme`'s publish,
//! `crate::controls::platform::resolve_font`) reads exactly as before.

pub(crate) use crate::coretext::*;
