//! Reactive-programming primitives for ForgeKit (phase 5.5 scaffold).
//!
//! This crate re-exports the `reactive_graph` types the later programming-model
//! tasks build on. It is currently a compiling stub: the smoke test below just
//! proves an `Owner` can be created, a signal built inside it, and its value
//! read back.

pub use reactive_graph::owner::{Owner, on_cleanup, provide_context, use_context};
pub use reactive_graph::signal::RwSignal;

#[cfg(test)]
mod tests {
    use super::*;
    use reactive_graph::traits::{Get, Set};

    #[test]
    fn owner_and_signal_round_trip() {
        let owner = Owner::new();
        owner.set();

        let signal = RwSignal::new(1);
        assert_eq!(signal.get(), 1);

        signal.set(2);
        assert_eq!(signal.get(), 2);
    }
}
