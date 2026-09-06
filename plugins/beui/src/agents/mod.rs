//! The agent-interface modules, one per ported beUI agent part — the pieces a
//! chat or tool-using assistant surface is assembled from (message bubbles,
//! prompt input, tool approval, streaming and activity states).
//!
//! Same two conventions as [`components`](crate::components): symbols are
//! module-prefixed, and the module list is fixed up front so adding one never
//! touches this file or `lib.rs`.

pub mod agent_activity;
pub mod ai_sidebar;
pub mod approval_card;
pub mod chat_app;
pub mod citations;
pub mod code_block;
pub mod file_diff;
pub mod image_generation;
pub mod loading_states;
pub mod message;
pub mod message_bubble;
pub mod message_scroller;
pub mod prompt_input;
pub mod streaming_response;
pub mod todo_list;
pub mod tool_approval;
pub mod tool_result;
