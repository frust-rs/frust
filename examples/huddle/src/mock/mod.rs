//! **Deprecated migration shim** (huddle clean-architecture refactor, task 01
//! — `workflow/plans/features/huddle-clean-architecture/`): entity struct
//! definitions now live in their owning feature's `domain::entities`, and the
//! shared mock dataset now lives in [`crate::data::store`]. This module
//! re-exports the full former `mock` surface so every existing consumer keeps
//! compiling unchanged while tasks 02–05 migrate each feature slice onto the
//! new imports directly; task 06 deletes this module.

pub use crate::data::store::{
    FIREHOSE_COUNT, FIREHOSE_ID, activity, channel, channels, dms, firehose_messages, messages,
    messages_for, user, users,
};
pub use crate::features::activity::domain::ActivityItem;
pub use crate::features::channels::domain::{Channel, Dm};
pub use crate::features::messages::domain::{Message, MessageBody, Reaction};
pub use crate::features::profile::domain::{CURRENT_USER_ID, User, UserStatus};
