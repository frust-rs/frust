//! Hot-patch spike app: the `frust create` counter's `lib.rs` shape, moved into its own lib package so
//! `dx serve --hot-patch` replays it (see the spike's README.md).
//!
//! The root component's `build` is called directly by the desktop driver `frust::app!` generates, so
//! it never goes through the hot seam: everything a patch should change lives in the `component(..)`
//! children ([`home_page::HomePage`], [`counter_card::CounterCard`]), whose `build` frust-core routes
//! through subsecond's jump table under the `hotpatch` feature.

mod counter_card;
mod home_page;

use frust::{Component, View, component};

/// The root component: stateless config that hosts the home page.
#[derive(Default)]
pub struct SpikeApp;

impl Component for SpikeApp {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        component(home_page::HomePage {
            title: "Hot-patch spike".into(),
        })
    }
}

// The one-argument form: no design-system plugin, so no `setup` block. Generates `__frust_main` (and
// the mobile exports) in this crate; `runner` calls it.
frust::app!(SpikeApp);
