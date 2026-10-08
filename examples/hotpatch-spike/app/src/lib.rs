//! Hot-patch spike app: the `frust create` counter's `lib.rs` shape, moved into its own lib package so
//! `dx serve --hot-patch` replays it (see the spike's README.md).
//!
//! Under the `hotpatch` feature every component's `build`, the root included, goes through the hot
//! seam: the driver `frust::app!` generates calls `__frust_root_build`, which reaches the root's
//! `build` via `build_erased`, and the `component(..)` children ([`home_page::HomePage`],
//! [`counter_card::CounterCard`]) are routed the same way, through the installed jump table.

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
