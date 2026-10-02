# frust-paths

Shared platform-directory resolution and atomic-write helpers for Frust. It resolves data and cache directories per platform (XDG on Linux, `Library/Application Support` on macOS and iOS, `%APPDATA%` on Windows, the app files directory on Android) and provides atomic file writes. It is a leaf crate with no `frust-*` dependencies.

It is used by Frust's desktop shell and by plugins that store data. Applications usually do not depend on it directly.

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
