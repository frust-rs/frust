//! [`TextStyle`]: the minimal v1 styling knobs for a laid-out string.

use peniko::Color;

/// Styling applied uniformly to a laid-out string.
///
/// v1 covers only size and color; font family (currently the platform system
/// UI font), weight, and rich per-range styling are deferred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size in logical pixels.
    pub size: f32,
    /// Fill color for the glyphs.
    pub color: Color,
}

impl TextStyle {
    /// A style with the given `size` and `color`.
    pub fn new(size: f32, color: Color) -> Self {
        Self { size, color }
    }
}

impl Default for TextStyle {
    /// 16px opaque black.
    fn default() -> Self {
        Self {
            size: 16.0,
            color: Color::BLACK,
        }
    }
}
