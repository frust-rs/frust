# web-spike

Throwaway spike for the web-shell plan's Phase 0 (task w0-02): probes whether
the frust render-engine graph — `frust-scene`, `frust-text`, `frust-gpu`,
`frust-engine`, `frust-render`, all reached by **path** dependency — compiles
for `wasm32-unknown-unknown`, and records the exact flags/cfgs it needs.

**Read [`RESULTS.md`](RESULTS.md) first.** Short version: the answer is *not
yet*. No RUSTFLAGS, no `--cfg` and no feature flag are needed for the graph —
that part of the question is settled and the answer is "none" — but
`crates/frust-gpu/src/pipeline.rs` demands `Send + Sync` on wgpu handle types
for its threaded pipeline warm-up, and wgpu implements neither on
`wasm32-unknown-unknown`. That is 91 compile errors, all in that one file.
`RESULTS.md` § 4 carries a validated one-line remedy (in `frust-gpu`'s
manifest, outside this task's write scope) and proof that it turns the whole
graph green.

Not a shipping example: a standalone workspace (own `[workspace]` root, own
`Cargo.lock`), excluded from the root workspace graph via the root
`Cargo.toml`'s `[workspace]` `exclude` list, so its wasm32-only probe never
resolves into the native workspace's lockfile — same shape as
`examples/huddle`.

`frust-reactive` is deliberately NOT a dependency here: its `tokio`
multi-thread runtime init fails on `wasm32-unknown-unknown` (a later task,
w0-04, lands that arm), so `src/main.rs` is a render-less stub. It builds a
scene, compiles it to engine strips on the CPU, and constructs the GPU
context types, but never drives a live device or a browser canvas — that is
w0-03's job.

## Running the probe

```sh
cd examples/web-spike
cargo check --target wasm32-unknown-unknown
```

No environment prefix, no RUSTFLAGS. Needs the `wasm32-unknown-unknown`
rustup target (`rustup target add wasm32-unknown-unknown`). Offline-capable
once the lockfile's crates are cached; the first ever wasm32 resolution on a
host has to fetch `wgpu-core-deps-wasm`, which no native build pulls.

Expect it to **fail** at `frust-gpu` until the `RESULTS.md` § 4 remedy lands.

## `Cargo.lock`

Committed, following the `examples/huddle` precedent (`git ls-files
examples/huddle/Cargo.lock` shows it tracked there too): a standalone
workspace's lockfile is part of its own reproducible build, not the root
workspace's.

## Files

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest — path deps onto the five engine-graph crates plus `kurbo`/`peniko` directly (no `wgpu`/`wasm-bindgen`/`web-sys` row of its own; see its header comment). |
| `src/main.rs` | The render-less compile probe itself. |
| `index.html` | Static host-page skeleton for w0-03 (the browser render probe); not exercised by this task. |
| `RESULTS.md` | Full findings: the flags table, the blocker's diagnosis, the validated remedy, toolchain facts. |
