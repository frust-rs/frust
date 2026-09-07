//! `Base`-design cases — the framework's own baseline widgets, with no
//! design-system plugin involved (see [`crate::case::Design::Base`]).
//!
//! Each catalog category gets its own submodule; every case is a single
//! `pub(super) const <NAME>: Case` value (not a per-module slice — see
//! `basics.rs`/`text.rs`/`layout.rs`/`styling.rs`), and this module's own
//! [`CASES`] is one literal array naming every case across every submodule.
//! `button` is still the g1-02 seed case, inlined here rather than in its
//! own submodule — g1-04's `interaction` module replaces it.
//!
//! Concurrent case-batch tasks (g1-04: input/interaction/navigation/
//! scrolling; g1-05: animation/assets/painting) each own their own
//! submodule file and do not touch this file's existing lines — see the two
//! marked spots below for where their `mod` declaration and `CASES` entries
//! go.

use frust_core::{AnyView, any};
use frust_widgets::button;

use crate::case::{Case, Design};

mod basics;
mod layout;
mod styling;
mod text;
// g1-04 adds here: mod input; mod interaction; mod navigation; mod scrolling;
// g1-05 adds here: mod animation; mod assets; mod painting;

/// Seed case (g1-02) — g1-04's `interaction` module replaces this with the
/// real `button` case; left in place until then.
fn button_case() -> AnyView<()> {
    any(button("Click me", |_state: &mut ()| {}))
}

const BUTTON: Case = Case {
    slug: "button",
    title: "Button",
    size: Case::DEFAULT_SIZE,
    scale: Case::DEFAULT_SCALE,
    time_ms: Case::DEFAULT_TIME_MS,
    design: Design::Base,
    build: button_case,
};

/// This module's slice of the registry [`crate::cases`] concatenates: every
/// case constant across every submodule, plus the still-seeded [`BUTTON`]
/// case above.
pub const CASES: &[Case] = &[
    BUTTON,
    // basics (g1-03)
    basics::ANY_VIEW,
    basics::CONTAINER,
    basics::SAFE_AREA,
    basics::SCAFFOLD,
    // text (g1-03)
    text::TEXT,
    // layout (g1-03)
    layout::ALIGN,
    layout::DIVIDER,
    layout::FLEX,
    layout::PADDING,
    layout::SIZED_BOX,
    layout::STACK,
    // styling (g1-03)
    styling::ALIGNMENT,
    styling::COLOR,
    styling::EDGE_INSETS,
    styling::TEXT_STYLE,
    // g1-04 adds here: input::*, interaction::*, navigation::*, scrolling::*
    // g1-05 adds here: animation::*, assets::*, painting::*
];
