# frust-i18n-macros

Compile-time Fluent message bundle loader proc macro for `frust-i18n`.

This is a proc-macro crate that provides the `locales!` macro used by `frust-i18n` to discover, validate, and compile Fluent message bundles at build time. **Applications do not depend on this crate directly** — `frust-i18n` re-exports the `locales!` macro at its crate root, so you only need to add `frust-i18n` as a dependency.

For usage documentation, see the [frust-i18n README](../README.md) and the [Frust architecture documentation](https://frust.dev).

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option. See the [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT) files for details.
