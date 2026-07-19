//! `channels` presentation layer (huddle clean-architecture refactor, task 02):
//! the roster [`ChannelsController`](controllers::ChannelsController) and the
//! Home tab's pages ([`home`](pages::home), [`workspace_drawer`](pages::workspace_drawer)).
//! Imports domain only — never `data/`.

pub mod controllers;
pub mod pages;

pub use controllers::ChannelsController;
