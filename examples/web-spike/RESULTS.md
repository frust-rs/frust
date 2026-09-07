# w0-02 results: `wasm32-unknown-unknown` compile probe of the engine graph

## Verdict: probe COMPLETE — and the engine graph does **NOT** yet compile for `wasm32-unknown-unknown`

The probe ran to completion and produced a definitive answer. Every crate in
the graph *except one* compiles clean for `wasm32-unknown-unknown` at wgpu
30.0.1 with `webgpu` + `webgl` both on, with **no RUSTFLAGS, no `--cfg`, and
no feature flag of any kind**. The single blocker is in `frust-gpu` itself,
not in wgpu, not in the manifest, and not in this spike:

> **`crates/frust-gpu/src/pipeline.rs` requires `wgpu::RenderPipeline: Send + Sync`
> for its threaded pipeline warm-up. wgpu does not implement `Send`/`Sync` for
> its types on `wasm32-unknown-unknown`. 91 `E0277` errors, all in that one file.**

The remedy is a **one-line change in `crates/frust-gpu/Cargo.toml`** — a file
outside this task's write scope (it is w0-01's file). It has been **validated
empirically** here (see § 4): with the remedy applied, the whole graph goes
green in one pass. This spike therefore hands the conductor a settled
diagnosis plus a proven fix, and deliberately leaves the fix itself unlanded.

## 1. The flags table (the card's actual question)

Every row below is empirical — observed from a completed `cargo check`, a
completed `cargo tree`, or the committed `Cargo.lock` — not inferred.

| Question | Finding | Evidence |
|---|---|---|
| Is `--cfg=web_sys_unstable_apis` required? | **NO** — confirmed, as the card predicted. wgpu 30.0.1 vendors its WebGPU bindings under `src/backend/webgpu/webgpu_sys/` rather than going through `web-sys`'s unstable-API surface. `wgpu`, `wgpu-hal`, `wgpu-core`, `wgpu-types`, `naga` and `web-sys` all compiled for wasm32 with an empty RUSTFLAGS. | § 4's green run: `wgpu`/`wgpu-hal`/`wgpu-core`/`web-sys` all reached `Checking`/`Compiling` and finished with no cfg set. |
| Does `getrandom` enter the wasm32 graph? | **NO.** `getrandom` is not in the graph at any version. No `wasm_js` feature and no `--cfg getrandom_backend="wasm_js"` flag is needed for this crate set. | `cargo tree --target wasm32-unknown-unknown -i getrandom` → `error: package ID specification 'getrandom' did not match any packages`; `grep -c getrandom Cargo.lock` → `0`. |
| Any other RUSTFLAGS / `--cfg` needed? | **NONE.** The entire probe — both the red run and the green remedy run — used a completely empty RUSTFLAGS. No linker flag, no `-C target-feature`, no `--cfg` of any kind. | Both runs in § 3 and § 4 are the bare command with no environment prefix. |
| Does the graph compile as-is at base `34f1306a`? | **NO** — 91 × `E0277` in `crates/frust-gpu/src/pipeline.rs`. See § 3. | § 3. |
| Does it compile with the validated remedy? | **YES** — clean, zero warnings, zero errors, all five crates. | § 4. |
| `wgpu` identity | **Single identity**, `wgpu 30.0.1`, matching the workspace's native pin exactly (`wgpu-core` / `wgpu-hal` / `wgpu-types` / `naga` all `30.0.1`). The `webgpu` + `webgl` backend features are contributed by **exactly one** edge — `frust-gpu`'s target-gated row from w0-01. `frust-engine` and `frust-render` also depend on `wgpu`, but contribute only `std`/`parking_lot`; neither names a backend. This is the intended shape. | `cargo tree --target wasm32-unknown-unknown -i wgpu -e features`. |
| `wasm-bindgen` edge | `wasm-bindgen 0.2.127` (exactly wgpu's own `^0.2.127`), with `wasm-bindgen-futures 0.4.77`, `js-sys 0.3.104`, `web-sys 0.3.104`. Reached only transitively — this spike declares no `wasm-bindgen`/`web-sys`/`js-sys` row of its own. | `Cargo.lock`. |
| Graph size | 179 packages. | `grep -c '^\[\[package\]\]' Cargo.lock`. |

## 2. Exact commands

Run from `examples/web-spike` (this directory), with no environment prefix:

```sh
cargo check --target wasm32-unknown-unknown          # the acceptance command
cargo tree  --target wasm32-unknown-unknown -i getrandom
cargo tree  --target wasm32-unknown-unknown -i wgpu -e features
```

And from the repository root, to prove non-interference:

```sh
cargo metadata --no-deps
```

No network access is required once the lockfile's crates are in the host's
Cargo registry cache; the check above is offline-capable. The first ever
wasm32 resolution on a given host must fetch `wgpu-core-deps-wasm` (wgpu's
wasm-only HAL shim, sibling of the `wgpu-core-deps-apple` /
`wgpu-core-deps-windows-linux-android` shims a native build already pulls),
which no native build ever downloads.

## 3. The blocker, in full

```
error: could not compile `frust-gpu` (lib) due to 91 previous errors
```

All 91 are `E0277`, all in `crates/frust-gpu/src/pipeline.rs`, and they split
57 × "cannot be sent between threads safely" / 34 × "cannot be shared between
threads safely". They point at four sites, all part of one mechanism:

| Site | Role |
|---|---|
| `pipeline.rs:755` | `VariantCache::<P>::warm_up`'s `where P: Send + Sync + 'static` bound. |
| `pipeline.rs:897` | `PipelineCache::warm_up` calling into it with `P = wgpu::RenderPipeline`. |
| `pipeline.rs:931`, `:935` | `compiler(..) -> impl Fn(&RenderPipelineDesc) -> wgpu::RenderPipeline + Send + 'static`, which captures a cloned `wgpu::Device`. |

**Root cause.** wgpu 30.0.1's `build.rs` defines its `send_sync` cfg alias as:

```rust
send_sync: { any(
    native,
    all(feature = "fragile-send-sync-non-atomic-wasm", not(target_feature = "atomics"))
) },
```

`native` is `not(target_arch = "wasm32")`. So on `wasm32-unknown-unknown`,
with that feature off, **no wgpu handle type is `Send` or `Sync`** — not
`Device`, `Queue`, `RenderPipeline`, `Surface`, `TextureView`, nor any of the
`dyn Dyn*` HAL trait objects the errors name. `frust-gpu` compiles a
background pipeline warm-up worker (`std::thread::Builder::spawn` at
`pipeline.rs:768`, plus a second `std::thread::spawn` at `:1264`) on **every**
target, unconditionally, so those bounds are demanded on wasm too.

This is a genuine pre-existing gap, not a defect in this spike: nothing in
the root workspace has ever been compiled for a wasm target before, so no
gate could have caught it.

## 4. The remedy — validated, deliberately NOT landed

Adding wgpu's `fragile-send-sync-non-atomic-wasm` feature to the wasm32 arm
makes the whole graph compile. Proven by temporarily adding a direct wgpu row
carrying that feature to this spike's own manifest, re-running the acceptance
command, and then **reverting the row** (the committed `Cargo.toml` has no
`wgpu` row, and the committed `Cargo.lock` is byte-identical to its
pre-experiment state):

```
Compiling wgpu-hal v30.0.1
 Checking wgpu-types v30.0.1
Compiling wgpu-core v30.0.1
Compiling wgpu v30.0.1
 Checking wgpu-naga-bridge v30.0.1
 Checking wgpu-core-deps-wasm v30.0.1
 Checking frust-gpu v0.1.0 (.../crates/frust-gpu)
 Checking frust-engine v0.1.0 (.../crates/frust-engine)
 Checking frust-render v0.1.0 (.../crates/frust-render)
 Checking web-spike v0.1.0 (.../examples/web-spike)
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.01s
```

Zero errors, zero warnings, all five graph crates plus the spike binary. So
`pipeline.rs` is the **only** wasm32 blocker in the whole engine graph — there
is no second wall behind it.

The row was reverted rather than kept because the fix belongs in
`crates/frust-gpu/Cargo.toml`, not here: a spike must not be the thing that
enables a feature the framework crate needs (every later web consumer — w0-04
onward — would have to re-enable it), and the Version-Pin Policy keeps `wgpu`
reachable from this manifest only transitively through `frust-gpu`.

### Two candidate landings, for the conductor to choose

**(A) Feature-only — one line, in `crates/frust-gpu/Cargo.toml`:**

```toml
[target.'cfg(target_arch = "wasm32")'.dependencies]
wgpu = { workspace = true, features = ["webgpu", "webgl", "fragile-send-sync-non-atomic-wasm"] }
```

This is the line that was validated above. It is sound on this target: the
`Send`/`Sync` impls it adds are "fragile" precisely because they are unsound
*if* the values really do cross a thread, and on `wasm32-unknown-unknown`
(no atomics, no `std` thread support) nothing ever can. wgpu itself gates the
feature on `not(target_feature = "atomics")` for exactly that reason, so
enabling it can never take effect on a future threads-enabled wasm build.

Runtime consequence, which is **benign but worth knowing**: `warm_up`'s
`std::thread::Builder::spawn` will fail on wasm (std has no thread support on
this target), and `pipeline.rs:770-778` already handles that — it logs
`could not spawn the pipeline warm-up thread (…); building the listed
variants inline` and drains the queue on the calling thread. So warm-up stays
*correct*; it just becomes synchronous. On a browser that means pipeline
compilation runs on the main thread, a jank risk for w0-03 to measure, not a
correctness bug. (Compile-checked here; the runtime path is unverified — this
spike drives no browser.)

**(B) Structural — cfg-gate the threaded warm-up off on wasm32**, so the
inline path is taken deliberately rather than via a failed spawn, and no
"fragile" `Send`/`Sync` lie is introduced at all. Cleaner, but a real code
change to `pipeline.rs`, and it needs its own review.

**Recommendation:** land **(A)** now to unblock w0-03 — it is one line, it is
validated, and it is self-limiting on threads-enabled wasm — and file **(B)**
as a follow-up once w0-03 has measured whether inline pipeline compilation
actually janks a browser frame. Either way this is a **conductor decision on a
file outside w0-02's write scope**; w0-02 changed nothing in `crates/`.

## 5. Root-workspace non-interference

- `cargo metadata --no-deps` at the repository root: **exit 0**, and the root
  workspace does not see this package at all (`web-spike` appears **0** times
  in its output) — the `exclude` row added at base commit `34f1306a` works.
- `git status --porcelain` shows nothing outside this directory, and nothing
  inside it beyond the five declared files plus `Cargo.lock` (committed, per
  the `examples/huddle` precedent — `git ls-files examples/huddle/Cargo.lock`
  confirms huddle tracks its own lockfile too).
- No `target/` directory is created inside the worktree: this host's
  `~/.cargo/config.toml` sets a global `target-dir = "/data/cache/target"`,
  so build artifacts never land in the checkout.

## 6. Toolchain facts (environment-supplied, unmodified)

| Tool | Version | Note |
|---|---|---|
| rustc / rustup toolchain | `1.98.1` (`rustc 1.98.1 (48a229cea 2026-09-01)`) | Repo-pinned via `rust-toolchain.toml`. |
| `wasm32-unknown-unknown` target | installed | `rustup target list --installed`. |
| `wasm-bindgen-cli` | `0.2.128` | **One patch ahead** of the resolved `wasm-bindgen` crate (`0.2.127`) — see below. |
| `wasm-pack` | `0.15.0` | Not exercised here (no `.wasm` is built by w0-02). |
| `wasm-opt` | `130` | Not exercised here. |

None of the above were installed, upgraded, or otherwise modified by this
task.

**Flag for w0-03:** `wasm-bindgen-cli 0.2.128` vs. crate `0.2.127` is a
one-patch mismatch. Both satisfy wgpu's `^0.2.127`, but wasm-bindgen's tooling
contract is stricter than semver — the CLI embeds a schema version that must
match the crate's, and a mismatch surfaces when `wasm-bindgen`/`wasm-pack`
post-processes a built `.wasm`, **not** at `cargo check`. It is therefore
untested by this task. w0-03 should expect either to pin the crate to
`0.2.128` or to use a `0.2.127` CLI, and should not lose time rediscovering
this.

## 7. What this spike deliberately does not do

- **No `frust-reactive`**: its `tokio` multi-thread runtime init fails on
  wasm32. w0-04 lands that arm; `src/main.rs` is render-less because of it.
- **No browser, no `.wasm` artifact, no `wasm-pack` run**: that is w0-03.
  `index.html` is only a skeleton for w0-03 to build on.
- **No live GPU device**: `RenderContext::new` and `SurfaceRenderer::new`
  enumerate no adapter, so the probe type-checks real call sites without
  needing a canvas. `SceneCompiler::compile` is the deepest real engine call
  reachable without one — it produces strips on the CPU.

## 8. Files in this spike

| File | Purpose |
|---|---|
| `Cargo.toml` | Standalone workspace manifest; path deps onto the five engine crates plus `kurbo`/`peniko`. No `wgpu`/`wasm-bindgen`/`web-sys`/`js-sys` row. |
| `src/main.rs` | The render-less probe: real call sites into all five crates. |
| `index.html` | Static host-page skeleton for w0-03. |
| `README.md` | How to run the probe. |
| `RESULTS.md` | This file. |
| `Cargo.lock` | Committed, per the `examples/huddle` precedent. |
