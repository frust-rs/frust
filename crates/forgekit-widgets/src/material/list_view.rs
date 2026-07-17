//! The virtualized `ListView` (Phase 6c, PLAN.md D4/D5, task 12):
//! `ListView::builder(item_count, item_extent, |index| -> AnyView)` with a
//! uniform, required `item_extent` (variable-extent lazy layout is deferred).
//! Windowed materialization happens at `View::rebuild` time — the widget
//! caches `viewport: Size` from the previous layout pass and reads scroll
//! offset from the retained element, keying visible children by item index
//! (`ChildKey`) so a window shift relocates surviving children instead of
//! rebuilding them.
//!
//! A doc-only stub today; task 12 fills this module.
