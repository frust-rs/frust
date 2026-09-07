//! Browser input on a focusable canvas (w1-04 owns this module).
//!
//! winit 0.30.13's web backend emits separate mouse and touch variants (no
//! unified pointer API at this pin), wheel deltas in line or pixel units, and
//! plain keyboard events; IME/composition is Phase 3's hidden-input overlay.
//! This module is seeded empty by the conductor so the crate's module tree
//! exists ahead of the card that fills it in; nothing here is public API yet.
