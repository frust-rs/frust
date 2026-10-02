# frust-scene

Layer 3 of Frust: the vector scene and display list. `Scene`, `SceneBuilder` and `Command` are the seam between widgets and the GPU backend. Only `kurbo` (geometry) and `peniko` (brushes and fonts) appear in this crate's public API, so the render backend is not tied to a specific GPU library.

Applications reach these types through the `frust` facade; they do not usually depend on `frust-scene` directly.

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
