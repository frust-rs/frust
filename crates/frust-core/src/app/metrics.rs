//! [`WindowMetrics`] and [`Orientation`]: shell-provided window-shape context; `RenderRoot` never
//! stores or reads it.

use kurbo::Size;

use crate::insets::WindowInsets;

/// The window's logical size, device-pixel scale, derived [`Orientation`] and [`WindowInsets`],
/// as a plain `provide_context` value: `use_context` subscribes to nothing, so a changed value is
/// visible only on the next rebuild. Re-provide it only on change, beside the standalone insets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    /// The window's logical (density-independent) size.
    pub size: Size,
    /// The device-pixel scale factor (logical → physical px multiplier).
    pub scale: f64,
    /// Derived from `size`; no platform callback carries an orientation.
    pub orientation: Orientation,
    pub insets: WindowInsets,
}

impl WindowMetrics {
    /// Construct a [`WindowMetrics`], deriving [`orientation`](Self::orientation) from `size`.
    pub fn new(size: Size, scale: f64, insets: WindowInsets) -> Self {
        Self {
            size,
            scale,
            orientation: Orientation::from_size(size),
            insets,
        }
    }
}

/// A window's derived portrait/landscape orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// `size.height >= size.width`, including the exact-square case.
    Portrait,
    /// `size.height < size.width`.
    Landscape,
}

impl Orientation {
    /// Portrait when `height >= width` (an exact square reads as portrait), landscape otherwise.
    pub fn from_size(size: Size) -> Self {
        if size.height >= size.width {
            Orientation::Portrait
        } else {
            Orientation::Landscape
        }
    }
}
