# frust-mcp

An MCP (Model Context Protocol) server that exposes Frust app control and diagnosis to AI agents. It serves the Streamable HTTP transport on `127.0.0.1` only; the bind address is not configurable.

The crate has two halves: a session engine that launches and supervises app sessions through `frust-drive` (logs, devtools connections, frame stats, metrics), and a tool layer on top of it covering session lifecycle, app driving and diagnosis.

```rust,ignore
let config = frust_mcp::McpConfig::new(std::env::current_dir()?);
frust_mcp::run(config).await
```

It is a library with no binary of its own; `frust-tui` embeds it so an agent drives the same sessions the workbench shows. Applications do not depend on it, and it is not part of the `frust` facade.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this crate.
