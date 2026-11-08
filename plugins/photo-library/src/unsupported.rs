//! Targets with neither a photo picker nor a native file dialog (web and other).

use crate::{PhotoError, PickOptions, PickedItem};

pub(crate) fn pick(_options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    Err(PhotoError::Unsupported)
}
