# material3-demo

The `frust-material` catalog's desktop gallery: a frust port of the
`material_3_expressive` package's own example app. Five sections (Do / Pick /
View / Nav / Find) list 39 component entries; opening one shows that
component's playground.

```bash
cargo run -p material3-demo
```

A root-workspace member (see `Cargo.toml`'s header), so the standard verify
gate covers it:

```bash
cargo test -p material3-demo \
  && cargo clippy -p material3-demo --all-targets -- -D warnings \
  && cargo fmt --check
```

## Shape

| Piece | Where |
|-------|-------|
| Gallery shell (app bar + navigation bar), routes | `src/main.rs` |
| Section/entry metadata and the 39-entry catalog | `src/catalog/` |
| Adaptive section host, list pane, pushed-route chrome | `src/pages/` |
| One file per catalog entry | `src/pages/playground/{do_,pick,view,nav,find}/` |
| Theme settings (seed, brightness, font, type style) | `src/theme/` |
| Shared playground widgets | `src/widgets/` |

The host is adaptive at 900px, like the reference: below it a row push a
full-screen playground route; at or above it the section shows a 320px list
pane beside the selected playground.

`assets/i1.png`..`i6.png` are the reference app's demo images (carousel, cards).
The bundled Roboto Flex/Mono faces are **not** copied here —
`frust_material::install()` registers them already.
