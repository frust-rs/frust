//! `weight_probe` — faux-bold device probe (not a PROTOCOL frame/d-class scenario).
//!
//! Draws one short sample string at 44 px in four rows (400, 700, 400 italic,
//! 700 italic) in the bundled static `testing/fonts/NotoSans-Subset.ttf`: weight
//! class 400, no `fvar`, no bold or italic sibling. The oracle's selector
//! therefore has to synthesize embolden for a 700 request (and skew for italic),
//! exercising the unhinted upem basis on the mobile tier rather than a
//! variation-coordinate weight axis. A device run counts ink per row band: the
//! 700 rows must carry clearly more ink than the 400 rows.
//!
//! The sample is `Hello, Hello`; the subset's cmap covers exactly space, `,`,
//! `H`, `e`, `l`, `o`, `é` and the combining circumflex U+0302, so every glyph in
//! the sample is real (no `.notdef` boxes). No lowercase `h` is mapped.
//!
//! The font is compiled in and registered by [`register_font`], which
//! `BenchState::new` calls from `BenchApp::init`: that runs before the shell's
//! construction-time drain, whereas a registration made in `build` lands after
//! it and a static scene requests no later frame to apply it. The family name
//! is the font's own (`Noto Sans`); no other scenario names it.

use std::sync::Once;

use frust::authoring::text::{FontFamily, FontWeight};
use frust::{AnyView, Color, any, column, text};

use super::{BenchState, Scenario};

/// The static subset font (path relative to this source file).
const FONT: &[u8] = include_bytes!("../../../../testing/fonts/NotoSans-Subset.ttf");

/// The family name the subset registers under (its name table's family).
const FAMILY: &str = "Noto Sans";

/// The sample string; every character is in the subset's cmap.
const SAMPLE: &str = "Hello, Hello";

/// Glyph size in logical pixels.
const SIZE: f32 = 44.0;

/// The four rows: (weight, italic).
pub const ROWS: [(FontWeight, bool); 4] = [
    (FontWeight::REGULAR, false),
    (FontWeight::BOLD, false),
    (FontWeight::REGULAR, true),
    (FontWeight::BOLD, true),
];

/// Register the subset font process-wide, once. Called from `BenchState::new`
/// so the shell's construction-time font drain applies it.
pub(crate) fn register_font() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| frust::register_app_fonts(FONT.to_vec()));
}

/// The weight probe scenario.
pub struct WeightProbe;

impl Scenario for WeightProbe {
    fn id(&self) -> &'static str {
        "weight_probe"
    }

    fn title(&self) -> &'static str {
        "Faux-bold weight probe (static Noto Sans subset)"
    }

    // erasure: keep Scenario::build feeds the dyn SCENARIOS table
    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        let mut col = column();
        for (weight, italic) in ROWS {
            let mut row = text(SAMPLE)
                .size(SIZE)
                .family(FontFamily::named(FAMILY))
                .weight(weight)
                .color(Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xFF));
            if italic {
                row = row.italic();
            }
            col = col.child(row);
        }
        any(col)
    }
}
