# frust-devtools

The in-app debug service a Frust app hosts for external tooling. It listens on an ephemeral loopback TCP port (`127.0.0.1` only), speaks the NDJSON JSON-RPC protocol defined by `frust-devtools-protocol`, and answers every request through one trait, `DevtoolsBackend`, which a shell implements. The service mints a random per-process token, prints it on a discovery line, and requires it at `handshake` before any other method is served.

The tooling side (`frust-drive`, `frust-tui`) does not depend on this crate; both sides meet at `frust-devtools-protocol`. The only Frust crate this one depends on is that protocol crate.

Applications do not depend on this crate directly. The platform shells embed it behind their `devtools` feature, which is meant for debug and profile builds.

```rust,ignore
let devtools = Service::start(ShellBackend, AppInfo::new("my-app", "0.1.0"))?;
// ...each frame, from the frame hook (never blocks):
// devtools.publish_frame_stats(stats);
devtools.shutdown();
```

`ShellBackend` is a type implementing `DevtoolsBackend`; see the crate documentation for a complete example.

Documentation and project home: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option (SPDX: `MIT OR Apache-2.0`).
