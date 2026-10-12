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
frust = { package = "frust-ui", version = "0.6" }
frust-material = "0.6"
```

The package is named `frust-ui` and imported as `frust`, because the crates.io name `frust` belongs to another project (<https://github.com/lloydmeta/frunk/issues/258>). The dependency key is what `frust run` and `frust build` use to enable `frust/devtools`-style features: frust-cli 0.6.0 requires the key `frust`, while newer CLIs resolve any key, so with them `frust-ui = "0.6"` also works.

A counter app, the shape `frust create` generates. `build` reads as one declarative tree, and
`frust::app!` turns the `Component` into the platform entry points:

```rust
use frust::{Align, Alignment, Component, CrossAxisAlignment, View, column, icon, scaffold, text};
use frust_material::{AppBarContainer, app_bar, fab, icons};

#[derive(Default)]
pub struct CounterApp;

impl Component for CounterApp {
    type State = u32;

    fn init(&self) -> u32 {
        0
    }

    fn build(&self, count: &mut u32) -> impl View<u32> {
        scaffold(Align(
            Alignment::CENTER,
            column()
                .child(text("You have pushed the button this many times:"))
                .child(text(count.to_string()).size(48.0))
                .cross_axis(CrossAxisAlignment::Center),
        ))
        .app_bar(
            app_bar::<u32>("Counter")
                .container(AppBarContainer::InversePrimary)
                .elevation(2),
        )
        .fab(Self::increment_fab())
    }
}

impl CounterApp {
    fn increment_fab() -> impl View<u32> {
        fab(icon(icons::ADD), |count: &mut u32| *count += 1).label("Increment")
    }
}

// `setup` runs once on the UI thread before the root `init`. Material 3 is the
// design system; swap in another design-system plugin's `install()` to change it.
frust::app!(
    CounterApp,
    setup = {
        frust_material::install();
    }
);
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
