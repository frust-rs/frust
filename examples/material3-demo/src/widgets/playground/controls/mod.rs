//! Controlled playground knobs, one shape per file — the reference's
//! `widgets/playground/controls/`.
//!
//! Every control here is a **controlled component**: it reports the
//! *requested* value through its callback and never mutates the `value`/
//! `checked` it was handed, mirroring the underlying `frust_material` widget
//! it wraps (`docs/CODE_STANDARDS.md`'s Interaction Semantics).

mod enum_menu;
mod enum_segmented;
mod slider;
mod switch;
mod text_field;

pub use enum_menu::{play_enum_menu_field, play_enum_menu_panel};
pub use enum_segmented::play_enum_segmented;
pub use slider::play_slider;
pub use switch::play_switch;
pub use text_field::play_text_field;
