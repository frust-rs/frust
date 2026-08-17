//! The component modules, one per ported shadcn component.
//!
//! Every module here is re-exported **wholesale** from the crate root
//! ([`crate`]'s namespace note), so app code names one flat namespace —
//! `frust_shadcn::button(…)`, `frust_shadcn::ButtonVariant` — rather than a
//! module path per component. Two conventions keep that flat surface workable:
//!
//! - **Public symbols are component-prefixed**: `ButtonVariant`, `BadgeVariant`,
//!   `CardView`, never a bare `Variant` or `Size`. Forty-eight modules glob into
//!   one namespace; an unprefixed name is a collision waiting for the next
//!   component to land.
//! - **The module list is fixed up front.** Every planned component has a module
//!   from the start (empty until the widget lands), so adding a component never
//!   touches this file or `lib.rs`'s re-export block.

pub mod accordion;
pub mod alert;
pub mod alert_dialog;
pub mod aspect_ratio;
pub mod attachment;
pub mod avatar;
pub mod badge;
pub mod breadcrumb;
pub mod bubble;
pub mod button;
pub mod button_group;
pub mod card;
pub mod carousel;
pub mod checkbox;
pub mod collapsible;
pub mod combobox;
pub mod command;
pub mod context_menu;
pub mod dialog;
pub mod drawer;
pub mod dropdown_menu;
pub mod empty;
pub mod field;
pub mod hover_card;
pub mod input;
pub mod input_group;
pub mod input_otp;
pub mod item;
pub mod kbd;
pub mod label;
pub mod marker;
pub mod message;
pub mod message_scroller;
pub mod native_select;
pub mod pagination;
pub mod popover;
pub mod progress;
pub mod questionnaire;
pub mod radio_group;
pub mod resizable;
pub mod scroll_area;
pub mod select;
pub mod separator;
pub mod sheet;
pub mod sidebar;
pub mod skeleton;
pub mod slider;
pub mod spinner;
pub mod switch;
pub mod table;
pub mod tabs;
pub mod textarea;
pub mod toggle;
pub mod toggle_group;
pub mod tooltip;
