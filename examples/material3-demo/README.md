# material3-demo

The `frust-material` catalog's gallery: a frust port of the
`material_3_expressive` package's own example app. Five sections (Do / Pick /
View / Nav / Find) list 39 component entries; opening one shows that
component's playground.

```bash
cd examples/material3-demo
cargo run                # desktop preview
frust run -d <device>    # Android / iOS device or simulator
```

A **standalone workspace** (its own `[workspace]` root and `Cargo.lock`,
excluded from the root one — see `Cargo.toml`'s header), so it gates from its
own directory rather than with `-p` from the repo root:

```bash
cd examples/material3-demo
cargo test \
  && cargo clippy --all-targets -- -D warnings \
  && cargo fmt --check
```

The package is `material3demo` (no hyphen) even though the directory keeps
one: the crate name is the `.so`/`.a` leaf the Android and iOS shells load by
identifier.

## Shape

| Piece | Where |
|-------|-------|
| Gallery shell (app bar + navigation bar), routes, `frust::app!` | `src/lib.rs` |
| Desktop preview entry point (calls the generated `__frust_main`) | `src/main.rs` |
| Section/entry metadata and the 39-entry catalog | `src/catalog/` |
| Adaptive section host, list pane, pushed-route chrome | `src/pages/` |
| One file per catalog entry | `src/pages/playground/{do_,pick,view,nav,find}/` |
| Theme settings (seed, brightness, font, type style) | `src/theme/` |
| Shared playground widgets | `src/widgets/` |
| Generated Android / iOS projects | `android/`, `ios/` |

The host is adaptive at 900px, like the reference: below it a row push a
full-screen playground route; at or above it the section shows a 320px list
pane beside the selected playground.

`assets/i1.png`..`i6.png` are the reference app's demo images (carousel, cards),
`include_bytes!`-embedded at compile time, so they ship inside the `.so`/`.a`
with no runtime asset lookup. The bundled Roboto Flex/Mono faces are **not**
copied here — `frust_material::install()` registers them already.
