# frust-drive

The shared drive logic behind the `frust` tools: project scaffolding, the environment doctor, device discovery, and the run, build and clean pipelines for desktop, Android and iOS apps.

It was split out of `frust-cli` so that `frust-cli`, `frust-tui` and `frust-mcp` can use the same pipelines. It holds no `clap` surface and depends on no framework crate. The app templates used by scaffolding are embedded in the crate.

Applications do not depend on `frust-drive` directly, and it is not re-exported by the `frust` facade; it is a library for Frust tooling.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this crate.
