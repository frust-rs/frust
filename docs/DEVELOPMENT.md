# ForgeKit - Development Guide

## Prerequisites

- Rust 1.88+ (workspace `rust-version`), edition 2024.
- macOS with Metal for the GPU smoke gate (`forgekit-render`'s `--ignored`
  test); other platforms can build and run the non-GPU suite.
- No Docker, CI config, or `.env` setup exists in this repo yet.

## Build

```bash
cargo build --workspace --locked
```

`--locked` must always pass — it is part of the verify gate below and is how
manifest/lockfile drift is caught (see *Version-Pin Policy*).

## Run

```bash
# Desktop preview: opens the example app in a native window.
cargo run -p hello
```

`cargo run -p hello` is the manual visual gate for rendering changes — there
is no automated pixel-diff test yet, so a person must look at the window.

## Test

```bash
# Standard verify gate (run before considering any change done):
cargo build --workspace --locked \
  && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

**Manual/gated tests** (not part of the default `cargo test --workspace`
run — each requires local hardware or is slow, and is marked `#[ignore]`
with a reason):

```bash
# GPU smoke test (forgekit-render): needs a real Metal/Vulkan device.
cargo test -p forgekit-render -- --ignored

# Scaffold end-to-end test (forgekit-cli): compiles a freshly generated
# project's full dependency graph (winit/vello/wgpu) — ~30s cold.
cargo test -p forgekit-cli --test create_e2e -- --ignored
```

### Template development

`forgekit create` embeds `templates/app/` into the binary at compile time.
To iterate on template files without rebuilding the embedded copy, pass the
hidden, development-only `--template-dir <path>` flag to point at a
filesystem copy of the template tree instead.

## Version-Pin Policy

The rendering stack's versions in `[workspace.dependencies]` are pinned
deliberately, not floating:

- `vello = "0.9.0"` requires `wgpu ^29.0.3`; the workspace pins
  `wgpu = "29.0.3"` (currently resolving to `29.0.4`) even though `wgpu`
  30.x is the ecosystem-latest release — bumping `wgpu` independently of
  `vello` breaks the build. See `docs/spec.md` §8/§15 for the ecosystem
  status this pin is tracking.
- **Never run a blind `cargo update`.** If a manifest changes any pinned
  dependency, run `cargo generate-lockfile` and then confirm
  `cargo build --workspace --locked` still succeeds before committing.
- Use `cargo tree -d` to check for duplicate/divergent versions of a crate
  across the dependency graph after any manifest change.

## Known Issues

### Formatting

The codebase is formatted with `rustfmt` using default settings (no
`rustfmt.toml`). `cargo fmt --check` is part of the verify gate above.
