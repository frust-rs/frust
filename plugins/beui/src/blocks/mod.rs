//! The block modules, one per ported beUI block — the composed, screen-scale
//! pieces built out of several primitives (a command palette, a signup form, a
//! scheduler, a wallet card).
//!
//! Same two conventions as [`components`](crate::components): symbols are
//! module-prefixed, and the module list is fixed up front so adding one never
//! touches this file or `lib.rs`.

pub mod availability_scheduler;
pub mod bloom_menu;
pub mod command_palette;
pub mod dynamic_island;
pub mod expandable_action_bar;
pub mod expandable_tabs;
pub mod feedback_widget;
pub mod file_upload;
pub mod infinite_masonry;
pub mod knockout_bracket;
pub mod morphing_search;
pub mod morphing_tabs;
pub mod not_found;
pub mod notification_stack;
pub mod otp_input;
pub mod overflow_actions;
pub mod prediction_market;
pub mod project_folder;
pub mod signup_form;
pub mod swap;
pub mod swipeable_list;
pub mod wallet_card;
