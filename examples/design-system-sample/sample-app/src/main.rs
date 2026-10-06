//! The app half of the out-of-tree design-system proof: a minimal desktop app
//! that installs [`sample_design`] and renders its three widgets plus its
//! transition pattern.
//!
//! Two things this file is deliberately proving, beyond "it renders":
//!
//! 1. **No catalog is compiled.** This binary's `frust` dependency inherits
//!    `default-features = false` from the workspace manifest, so
//!    `frust::material::*`, `frust::cupertino::*`, `frust::glyph::*` and the
//!    bundled Glyph fonts do not exist in this build — referencing one is a
//!    hard `E0433` here, which is the check. The baseline widget set
//!    (`Column`/`text`/`SizedBox`), the authoring toolkit and `frust::motion`
//!    all remain, which is what makes an app like this possible at all.
//! 2. **The install seam is `app!`'s `setup` block.** `sample_design::install()`
//!    runs before the first frame and before `Component::init`; see that
//!    function's own docs for why neither `Component::init` nor
//!    `set_app_theme` is a valid alternative.

use frust::motion::switcher::pattern_switcher;
use frust::{Column, Component, SizedBox, View, any, text};
use sample_design::{SampleReveal, sample_badge, sample_chip, sample_panel};

/// Vertical rhythm between the panel's rows, in logical px.
const GAP: f64 = 12.0;

/// The app's retained state: how many times the chip has been pressed.
#[derive(Default)]
struct State {
    presses: u32,
}

#[derive(Default)]
struct SampleApp;

impl Component for SampleApp {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        // The switcher's key is the press count, so every press stages a
        // transition through the design system's own pattern.
        let status = pattern_switcher(
            state.presses,
            SampleReveal,
            text(format!("{} press(es)", state.presses)).size(15.0),
        );

        any(sample_panel(Column(vec![
            any(sample_badge("sample design system")),
            any(SizedBox(None, Some(GAP))),
            any(sample_chip("press me", |state: &mut State| {
                state.presses += 1;
            })),
            any(SizedBox(None, Some(GAP))),
            any(status),
        ])))
    }
}

frust::app!(
    SampleApp,
    setup = {
        sample_design::install();
    }
);

fn main() {
    __frust_main();
}
