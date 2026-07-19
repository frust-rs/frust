//! `messages` presentation layer (huddle clean-architecture refactor, task 03):
//! the feed [`MessagesController`](controllers::MessagesController) and the
//! channel/DM pages ([`channel_feed`](pages::channel_feed),
//! [`thread`](pages::thread)). Imports domain only — never `data/` (apart from
//! the documented composition-root fallback in [`controllers`]).

pub mod controllers;
pub mod pages;

pub use controllers::MessagesController;
