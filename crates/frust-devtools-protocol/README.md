# frust-devtools-protocol

The wire contract between an in-app Frust debug service (`frust-devtools`) and external tooling (`frust-drive`, `frust-tui`). Both sides depend on this crate, and only on this crate, to agree on message shapes. It is a leaf: `serde` and `serde_json` only, with no async runtime and no other Frust crate.

Messages are JSON-RPC 2.0 objects, one per newline-terminated line. `encode_line` and `decode_line` serialize and discriminate them, and `Incoming` reports whether a decoded line is a `Request`, `Response` or `Notification`. A server announces its port and per-process token with a discovery line built by `format_discovery_line` and parsed by `parse_discovery_line`.

Applications do not depend on this crate directly. It is reached through the debug service in `frust-devtools` and through the Frust tooling.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option (SPDX: `MIT OR Apache-2.0`).
