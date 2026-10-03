# frust-cli

The command-line tool for Frust, a Rust-native declarative UI framework. `cargo install frust-cli` installs a binary named `frust`.

```sh
cargo install frust-cli
frust --help
```

Commands, as listed by `frust --help`:

- `create`: scaffold a new app
- `doctor`: validate the Frust toolchain (Rust targets, NDK, Android SDK, Xcode)
- `devices`: list connected devices, emulators, and simulators
- `clean`: remove build outputs (cargo target dirs and Gradle/Xcode build dirs)
- `tui`: launch the TUI workbench for managing Frust projects
- `run`: build, install, launch and stream logs on a connected device; with no Android device selected it falls back to `cargo run`, and `-d web` builds for the browser and serves the result
- `build`: produce a distributable artifact, a release-signed APK/AAB via Gradle or an iOS app/IPA via `xcodebuild`

`frust create --frust-path <checkout>` builds the generated project against a checkout of the Frust repository instead of the crates.io release (for framework contributors). `frust create --no-sync` skips the one-time network step that wires the Android and iOS projects to the embeddings; `frust run` and `frust build` do that wiring later.

Running `frust` with no command in an interactive terminal opens the TUI workbench.

`frust-cli` is a thin `clap` front end over `frust-drive` (the shared pipelines) and `frust-tui` (the workbench). It is a tool, not a library: applications do not depend on it, and it is not part of the `frust` facade.

For a getting-started walkthrough see <https://frust.dev/docs/get-started>.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this crate.
