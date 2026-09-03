# Mac rig — raw evidence

Raw evidence for device rounds run on the MacBook rig (Apple M4, 10-core GPU, Metal 4;
macOS 26.6.2; Xcode 26.2; built-in Retina panel at 1800x1169 points, DPR 2). This
directory holds the per-round notes; `benchmarks/RESULTS.md` carries the narrative
sections for the perf rounds (macOS desktop stress, macOS window session).

## GPU-seam device round — Phase 9 (p9-d1, branch `feature/frust-engine-p9` @ `503a5572`)

The Phase-9 GPU seam (`Command::SceneTexture`, `ShaderQuad` on the engine, the
shared-encoder/depth contract, `frust::gpu`) had only Linux/Vulkan evidence (NVIDIA T400).
This round re-ran every seam suite on Metal — the macOS adapter and the iOS Simulator's
Apple2-feature-set adapter — plus the shadertoy window smoke. No source changed; all
commands below ran from the primary checkout at `503a5572` with a clean tree.

### macOS, Apple M4 (Metal) — gate `engine-p9-metal-macos-seam`

| Suite | Command | Result | Wall |
|---|---|---|---|
| frust-gpu `shared_encoder` | `cargo test -p frust-gpu --test shared_encoder -- --ignored` | 3/3 | 25.8 s |
| frust-engine `scene_texture` | `cargo test -p frust-engine --test shared_encoder --test scene_texture --test shader_quad -- --include-ignored --test-threads=1` | 4/4 | 38.3 s |
| frust-engine `shader_quad` | (same run) | 12/12 | 34.4 s |
| frust-engine `shared_encoder` | (same run) | 3/3 | 34.8 s |
| frust `gpu_seam` (facade, `gpu` feature) | `cargo test -p frust --features gpu --test gpu_seam -- --include-ignored` | 4/4 | 26.1 s |
| frust-testing `alpha_polarity` | `FRUST_GOLDEN_EXPECT_ADAPTER="Apple M4" FRUST_GOLDEN_EXPECT_BACKEND=metal cargo test -p frust-testing --test engine_goldens --test alpha_polarity -- --ignored` | 2/2 | 28.1 s |
| frust-testing `engine_goldens` | (same run; class `engine-metal-macos`, corpus + filter family vs `vello_cpu` and the committed baselines) | 2/2 | 44.9 s |

Adapter as reported by every device suite: `Apple M4`, `IntegratedGpu`, backend `Metal`,
subgroup 4..64. Zero wgpu validation or error lines in any run; `testing/goldens/` untouched
(no promotion, no baseline drift). The `shader_quad` run includes the `503a5572` regression
case (a target reaped on its own per-target clock is rebuilt, not frozen) and the resize-churn
case from `f2d5623d`.

### macOS, shadertoy window smoke — gate `engine-p9-metal-macos-shadertoy`

`examples/shadertoy` release build (`cargo build --release` in the standalone workspace),
launched with `FRUST_WINDOW_SIZE=1000x700 FRUST_LOG=info`; marker line
`frust-render tier=engine (… Bgra8Unorm into a Bgra8Unorm swapchain, 2000x1400, adapter
`Apple M4`)`. Driven through AccessKit (`osascript` named button presses + AX window
resizes) with `screencapture` frames; animation proven by hashing a 400x300px centre crop of
two frames 1 s apart.

Per shader, in this order: enter → two frames 1 s apart → AX resize to 1400x950 (two frames)
→ 700x520 → back to 1000x732 → `< Back` (menu idle, 3 s) → re-enter (two frames) → `< Back`
→ enter another shader for 8 s (~960 frames at 120 Hz, past `MAX_UNSEEN_FRAMES` = 120 and the
per-target reap window, so the hidden program's target is reaped while frames keep ticking)
→ re-enter (two frames) → `< Back`.

| Shader | Entered | At 1400x950 | 700x520 / restored | Re-shown after idle hide | Re-shown after reap | Clip / stale content |
|---|---|---|---|---|---|---|
| Neon rings | animates (114-120 fps) | animates, fills | fills | animates | animates, fresh frame | none |
| Palette sweep | animates | animates, fills | fills | animates | animates, fresh frame | none |
| Synthwave sunset | animates (118-120 fps idle rig) | animates, fills | fills | animates | animates, fresh frame | none |
| Glassy field | animates | animates, fills | fills | animates | animates, fresh frame | none |

"animates" = the centre-crop hash of the two frames differs; "fills" = the shader reaches all
four window edges under the HUD at the new extent (checked by eye on the captures). The
whole session logged zero wgpu validation or error lines. FPS readouts taken while the
Simulator arm's `xcodebuild`/`rustc` were running on the same machine (Synthwave 73-83 fps,
Glassy field 36-61 fps under that load) are not perf figures — this is a liveness + resize +
reap smoke, not a perf record.

Driving note: the shell's AccessKit tree reports every node at half its rendered bounds on
this DPR-2 panel, so coordinate clicks (`click at`) resolve to the wrong element; the
protocol used named presses (`click button "<name>" of group 1 of window 1`) and
`set size of window 1`, which are unaffected. Filed as `act_000001a0660301f4ZIshUQT5`.

### iOS Simulator, iPhone 16 (udid `B911C0D8-D2FE-4FEF-84C5-E0C574D1A8A8`, Xcode 26.2) — gate `engine-p9-ios-sim-seam`

Runner: `CARGO_TARGET_AARCH64_APPLE_IOS_SIM_RUNNER="xcrun simctl spawn B911C0D8-…"`.

| Suite | Command | Result |
|---|---|---|
| frust-testing `ios_sim` (the existing Simulator gate, re-run on the Phase-9 branch) | `cargo test -p frust-testing --target aarch64-apple-ios-sim --test ios_sim` | 2/2 — unit corpus matches the CPU goldens on the Simulator's Metal (0 px differ on every case) |
| shadertoy app on the Simulator (substitute for the unrun `shader_quad` suite) | `frust run -d B911C0D8-…` from `examples/shadertoy` (debug, `xcodebuild` → `simctl install`/`launch`), driven by CGEvent taps into the Simulator window, frames via `xcrun simctl io <udid> screenshot` | PASS — marker `tier=engine (… Bgra8Unorm swapchain, 1179x2556, adapter `Apple iOS simulator GPU`)`; Neon rings renders full-screen at 60 fps under the HUD; `< Back` → Palette sweep for 8 s (~480 frames, past the reap windows) → Neon rings re-entered animates on a fresh frame; Home (⇧⌘H) for 3 s → `simctl launch` resumes it animating; 0 runtime error/validation lines. This is the app-path proof that `ShaderQuad` (and the `SceneTexture` binding it rides on) works on the Simulator's Apple2 Metal — the suites themselves are still owed there (next row). |
| frust-engine `scene_texture` / `shader_quad` / `shared_encoder` | `cargo test -p frust-engine --target aarch64-apple-ios-sim --test scene_texture --test shader_quad --test shared_encoder --no-fail-fast -- --ignored --test-threads=1` | NOT RUN: 4/4, 12/12, 3/3 cases panic at the fixture's `request_device` — `LimitsExceeded { max_inter_stage_shader_variables: requested 16, allowed 15 }`. The fixtures hard-code `wgpu::Limits::default()`; the Simulator's Apple2 adapter ("Apple iOS simulator GPU", Metal) offers 15. Harness limitation, not a Metal deviation — `ios_sim` passes on the same adapter because it derives its device from the adapter's own limits. Filed as `act_000001a066075794naVzG3V9` (fixture follow-up; `EngineOracle::new` has the identical refusal, already noted in `ios_sim.rs`). |

### Deviations and findings

- **AccessKit bounds halved on HiDPI** (`act_000001a0660301f4ZIshUQT5`, major, SHELLS): the desktop shell
  publishes node bounds in logical pixels where AccessKit expects physical; every AX rect is
  1/scale-sized. Not a Metal deviation — a shell bug the round happened to expose.
- **Seam suites cannot run on the iOS Simulator** (`act_000001a066075794naVzG3V9`, minor): the
  `scene_texture`/`shader_quad`/`shared_encoder` fixtures request `wgpu::Limits::default()`
  (16 inter-stage variables) and the Simulator's Apple2 adapter allows 15. LIMITATIONS
  candidate for the docs card (`engine-ios-sim-seam-suites-unrun`): ShaderQuad/SceneTexture
  on the Simulator is proven only through the app path above, not by the suites.
- **Stale HUD caption** (`act_000001a0660318181YE65WUj`, minor): the shadertoy running screen
  still says "composited via vello's texture override".
- No Metal-specific rendering deviation was observed on either adapter: every macOS suite,
  the goldens on the `engine-metal-macos` class, and the app-path Simulator run agree with
  the Linux/Vulkan evidence.
