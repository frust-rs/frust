//! iOS: the system photo picker. Compiling stub; the picker lands with the iOS implementation.

use crate::{PhotoError, PickOptions, PickedItem};

pub(crate) fn pick(_options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    Err(PhotoError::Unsupported)
}
