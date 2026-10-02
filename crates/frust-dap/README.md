# frust-dap

A Debug Adapter Protocol (DAP) server for Frust, embedded in its host. There is no separate `frust dap` process: `serve_embedded` runs a loopback DAP listener on the host's tokio runtime, over the host's own `frust_mcp::SessionBackend`. An editor attached through DAP and the host's own interface therefore drive the same app sessions.

The crate never prints to stdout or stderr, because it runs inside a terminal workbench; diagnostics go through the `log` crate.

It depends on `frust-drive` and `frust-mcp`. `frust-tui` is its embedder. Applications do not depend on it, and it is not part of the `frust` facade.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this crate.
