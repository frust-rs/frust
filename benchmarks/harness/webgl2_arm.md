# Browser WebGL2 Arm — Written NO-GO (engine plan Phase 0)

**Status: NO-GO for this pass.** No browser frame-time numbers exist in this
repository for any Frust scenario, and none are invented here. This file is
the arm's record per `RESULTS.md`'s own discipline (a scenario with no
completed runs is documented, not filled with placeholder numbers) and per
this card's acceptance: the arm is present either with numbers or with this
pointer.

## Decision context

OPEN #1 (engine plan, decided 2026-08-29): **(b)** — this plan adds no wgpu
feature. The target-gated `wasm32` `gles`/`webgpu` section is owned by the
Web Shell plan, not this one. Everything
below documents why a measurement is currently blocked and what the nearest
desktop stand-in is; it does not open that gate.

## What is blocked, and why

1. **Root `Cargo.toml`'s `wgpu` row compiles neither `gles` nor `webgpu`.**
   `Cargo.toml:89` drops `gles` ("no shell ever surfaces a GLES-only device
   — vello requires compute shaders GLES can't provide") and `Cargo.toml:92`
   drops `webgpu` ("wasm target only, Frust ships no wasm shell"); the
   `wgpu = { … features = […] }` table these lines document sits at
   `Cargo.toml:97-104` and lists neither. Enabling either is itself a
   Version-Pin-Policy change (`docs/DEVELOPMENT.md` § Version-Pin Policy:
   "Pins are LAW: never bump one independently") gated behind OPEN #1 — not
   a change this NO-GO card is authorized to make.
2. **No wasm shell exists.** None of `crates/frust-shell-{desktop,macos,
   windows,linux,android,ios}` targets `wasm32-unknown-unknown`; there is no
   fourth shell to host a canvas surface. A spike scene needs one (scratch,
   outside the workspace) to have anywhere to run.
3. **`wasm-bindgen-test` is absent from `Cargo.lock`.** Confirmed by
   `grep -n wasm-bindgen-test Cargo.lock` returning no match at this SHA —
   the workspace has never resolved a wasm-target test harness, consistent
   with (1) and (2).

None of the three is a bug; all three are the documented, deliberate state
of the tree at `12a00155b1f69ebaef08f7f56f7720c04ff77c9d`.

## What upstream already does (reference only — not built here)

vello_hybrid ships its own WebGL2 browser example and already answers the
"can this render at all" question upstream, at pins this workspace shares:

- `sparse_strips/vello_hybrid/examples/wgpu_webgl/src/lib.rs:54-76`
  (upstream vello clone under `tmp/vello/`, read-only) requests
  `wgpu::Backends::GL`, `wgpu::Features::empty()`, and
  `wgpu::Limits { max_texture_dimension_2d, max_buffer_size,
  ..wgpu::Limits::downlevel_webgl2_defaults() }` — i.e. the WebGL2 device
  path is the `gles` wgpu backend under the downlevel limit profile, not a
  separate renderer.
- `sparse_strips/vello_sparse_tests/README.md:44-50` runs the upstream
  sparse-strip test corpus under WebGL2 via
  `wasm-pack test --headless --chrome --features webgl --release`, noting a
  minimum Clang major version of 20 on macOS (`README.md:45`) as a host
  requirement for that build.

Neither file is part of this repository's dependency graph or CI; they are
cited as upstream fact establishing that the WebGL2 path *is* exercised
somewhere in the vello project, not as something this workspace runs.

## Desktop stand-in (future work, not built by this card)

This card's own description records the planned Phase 2 substitute: an
engine-side `FRUST_ENGINE_DOWNLEVEL=1` knob that would request the same
`downlevel_webgl2_defaults()` limit profile against the desktop Metal
backend already in the dependency graph, so the WebGL2 *limits* (not the GL
backend itself) can be measured without a browser or a wasm target. This
knob does not exist yet at this SHA (`grep -rn FRUST_ENGINE_DOWNLEVEL` finds
nothing) — it is a forward reference for whichever card builds Phase 2, not
a deliverable of this one.

## Browser floor (estimate, not a measurement)

Per this card's own record: WebGPU reaches an estimated ~70% of browsers by
mid-2026, with WebGL2 as the fallback for the remaining share. This is
carried here as the stated planning estimate behind OPEN #1, not a sourced
statistic and not a number this repository measured — flagged as an
estimate so it is never read back as a benchmark result.

## Reproduction

- `Cargo.toml:89,92,97-104` — the feature trim, at this SHA.
- `git -C <repo> show 12a00155:Cargo.lock | grep -c wasm-bindgen-test` → `0`.
- Upstream citations above are read against the vendored clone at
  `tmp/vello/sparse_strips/{vello_hybrid/examples/wgpu_webgl,
  vello_sparse_tests}` on the machine this card ran on; that clone is not
  part of this repository.

## See also

- `benchmarks/RESULTS.md` — "vello_hybrid spike" section, Arm 7, points back
  at this file.
- `docs/DEVELOPMENT.md` § Version-Pin Policy — why enabling `gles`/`webgpu`
  is not a local decision.
- The Web Shell plan — owns the target-gated
  `wasm32` `gles`/`webgpu` section this card explicitly does not open.
