# frust-tui

A mouse-first terminal workbench for Frust apps, built on `ratatui` and `crossterm`. It detects a project, then lets you run, build and supervise app sessions and inspect their logs from the terminal.

The crate is organised as a message-driven engine (pure `update` over a model), a render layer, and a supervisor that manages sessions and project detection. It also embeds the `frust-mcp` and `frust-dap` servers over the same sessions it shows.

It is a library with no binary of its own: the `frust` command from `frust-cli` calls into it, both for bare `frust` and for `frust tui`. Applications do not depend on it, and it is not part of the `frust` facade.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this crate.
