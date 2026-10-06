# frust

Frust is a Rust-native, mobile-first declarative UI framework: write one `View` tree in Rust and
ship it to Android, iOS, macOS, Windows, Linux, and the web, rendered everywhere by the
frust-owned `frust-engine` GPU pipeline.

`frust` is the facade crate. Application code depends on this single crate; it exposes `run`, the
canonical app entry point, and curates the view, widget and reactive vocabulary from the
underlying framework crates (`frust-core`, `frust-widgets`, `frust-reactive`, `frust-theme` and
others).

Status: pre-1.0; APIs may change between minor versions.

## Platforms

- Android and iOS
- macOS, Windows and Linux (desktop shells)
- Web (WebGPU, with a WebGL2 fallback)

## Example

Add the dependency:

```toml
[dependencies]
frust = { package = "frust-ui", version = "0.5" }
```

The package is named `frust-ui` and imported as `frust`, because the crates.io name `frust` belongs to another project (<https://github.com/lloydmeta/frunk/issues/258>).

A counter-style component, as shown in the crate-level documentation:

```rust
use frust::{Component, View, text};

struct Counter;

impl Component for Counter {
    type State = i32;

    fn init(&self) -> i32 {
        0
    }

    fn build(&self, state: &mut i32) -> impl View<i32> {
        text(format!("count: {state}")).size(32.0)
    }
}

frust::run(Counter).unwrap();
```

## Getting started

Install the command-line tool and create a project:

```sh
cargo install frust-cli
frust create my_app
```

`frust create` writes the dependency above into the new project.

Design systems ship as separate plugin crates that an application depends on beside `frust`; the
facade itself carries only the baseline widget set.

## Links

- Documentation: <https://frust.dev>
- Source: <https://github.com/frust-rs/frust>

## License

Licensed under `MIT OR Apache-2.0`, at your option. See `LICENSE-MIT` and `LICENSE-APACHE` in this directory.
