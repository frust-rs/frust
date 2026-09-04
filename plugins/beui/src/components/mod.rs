//! The component modules, one per ported beUI component — the catalog's
//! interaction primitives (controls, overlays, navigation, motion effects).
//!
//! Two conventions keep the three sibling catalogs (`components`, [`agents`],
//! [`blocks`]) workable side by side:
//!
//! - **Public symbols are component-prefixed**: `ButtonVariant`, `TabsView`,
//!   never a bare `Variant` or `Size`. The three families cover overlapping
//!   ground (a swap control here, a swap block there), so an unprefixed name is
//!   a collision waiting for the next module to land.
//! - **The module list is fixed up front.** Every planned component has a module
//!   from the start, so adding one never touches this file or `lib.rs`.
//!
//! [`agents`]: crate::agents
//! [`blocks`]: crate::blocks

pub mod action_swap;
pub mod adaptive_stepper;
pub mod animated_badge;
pub mod animated_sidebar;
pub mod animated_toast_stack;
pub mod bottom_sheet;
pub mod bounce_sidebar;
pub mod bouncy_accordion;
pub mod button;
pub mod center_morph_modal;
pub mod checkbox;
pub mod combobox;
pub mod context_menu;
pub mod cylinder_carousel;
pub mod dock;
pub mod drawer;
pub mod expandable_control;
pub mod expanding_arrow_button;
pub mod file_tree;
pub mod input;
pub mod loader;
pub mod marquee;
pub mod morphing_modal;
pub mod multi_select;
pub mod number;
pub mod popover;
pub mod preview_rail;
pub mod pull_to_refresh;
pub mod radio;
pub mod range_slider;
pub mod scroll_animation;
pub mod select;
pub mod shader_background;
pub mod shared_layout_bg;
pub mod switch;
pub mod table;
pub mod tabs;
pub mod text_animation;
pub mod theme_toggle;
pub mod tilt_card;
pub mod tooltip;
pub mod wheel_picker;
