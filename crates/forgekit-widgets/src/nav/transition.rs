//! Page-transition animations for the [`navigator`](super::navigator) (task 03).
//!
//! Intentionally empty in task 02: the navigator core ships **instant** page
//! switches only. Task 03 fills this module with the transition vocabulary
//! (slide/fade/scale drivers advanced from `PaintCtx::frame_time`) and wires it
//! into `NavigatorWidget`'s reserved transition slot and per-page paint offset —
//! the seams task 02 already leaves in place. Pre-declared in
//! [`super`](super)'s module list so task 03 never edits `nav/mod.rs`.
