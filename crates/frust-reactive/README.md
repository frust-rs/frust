# frust-reactive

The reactive substrate for Frust. It owns the process-wide `ReactiveRuntime`: a background tokio runtime, an `any_spawner` executor that routes `spawn` to that runtime and `spawn_local` to a UI-thread task queue, a swappable `FrameWaker`, and the root reactive `Owner`. It re-exports the `reactive_graph` types Frust builds on, and holds the process-wide deep-link, back-press and menu-event sources that shells write into. It is a leaf crate with no `frust-core` dependency.

Applications use signals and effects through the `frust` facade; platform shells depend on `frust-reactive` directly.

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
