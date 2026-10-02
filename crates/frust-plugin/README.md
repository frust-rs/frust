# frust-plugin

The substrate Frust plugins build on to reach the host OS through FFI without per-plugin native code. It has two independent halves: an Android platform-handle slot that holds the `(JavaVM, application Context)` pair, written by the Android shell and read by plugins, and a desktop view-factory registry that plugins write to and a desktop shell reads. It is a leaf crate with no `frust-*` dependencies.

Plugin crates depend on `frust-plugin` directly; applications reach plugins, not this crate.

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
