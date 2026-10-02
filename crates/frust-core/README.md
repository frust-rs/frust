# frust-core

Frust's layer 1 and 2: the declarative `View` API, the retained `Widget` tree, box-constraint layout, and the rebuild, layout and paint pass skeleton. It follows the `View` lifecycle of `xilem_core` and a `tree_arena`-backed widget tree, but has no xilem or masonry dependency and no `unsafe` code. Each platform shell owns a `RenderRoot` that drives this crate.

Applications normally reach these types through the `frust` facade rather than depending on `frust-core` directly.

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
