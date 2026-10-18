# damage-probe

Does this backend + compositor honour wgpu's damage hint?

A standalone pure-wgpu + winit probe of [gfx-rs/wgpu#10152](https://github.com/gfx-rs/wgpu/pull/10152)
(`Queue::present_with_damage`). It is its own Cargo workspace, outside the root graph, and depends on
the PR head **by commit SHA from the base repository** — no fork, no pin change, nothing pushed anywhere.
`Cargo.toml` carries the one `rev` constant; change it when the PR moves or lands.

## The method

The whole frame is always rendered (the PR's contract: *"the texture must still contain the whole
frame"*). A 48 px square moves one step per frame across a static background; only the **hint** varies:

| `--damage` | What is reported | What a compositor that honours the hint shows |
|---|---|---|
| `none` | nothing (`Queue::present`) — the control | a clean frame |
| `exact` | the square's old **and** new bounds | a clean frame |
| `under` | the **new** bounds only — the vacated old position is deliberately unreported | **a trail of stale squares**: the old positions are never repainted on screen |

A backend that ignores the hint (Metal, DX12, a Vulkan driver without `VK_KHR_incremental_present`) shows
a clean frame in every mode. So `under` is the proof: a trail means the rects were consumed, a clean
frame means they were not — or that the compositor repaints everything anyway.

`--full-every N` reports the whole surface every Nth frame, which wipes the trail (proof that the trail is
the compositor's, not the probe's). `--step`, `--size`, `--fps` tune the motion; `WGPU_BACKEND`
(wgpu's own knob: `vulkan`, `gl`, `dx12`, `metal`) picks the backend; `RUST_LOG=info` prints the adapter,
the surface configuration, and on Vulkan whether the adapter advertises `VK_KHR_incremental_present`
(the PR enables it on the device whenever it is advertised). The green marker never moves.

## Build and run

```bash
cd testing/damage-probe
cargo build --release --locked          # first build fetches the PR head
RUST_LOG=info cargo run --release -- --damage under
```

The binary lands in `$CARGO_TARGET_DIR/release/damage-probe` when that variable is set (it is, on the
rig), else in `target/release/`.

### Linux, X11 on the NVIDIA T400 (the linux-native container, display `:20`)

Host-built, presented into the container's Xorg via the shared abstract socket:

```bash
/home/ed/Dev/dockur-machines/linux-native/run-on-desktop.sh \
  /data/cache/target/release/damage-probe --damage under --step 6 &
sleep 7; DISPLAY=:20 import -window root under.png
```

### Linux, Wayland (nested KWin on the same display)

```bash
XDG_RUNTIME_DIR=/run/user/1000 DISPLAY=:20 kwin_wayland --x11-display :20 \
  --socket wayland-probe --width 1100 --height 800 --no-lockscreen &
env -u DISPLAY WAYLAND_DISPLAY=wayland-probe WAYLAND_DEBUG=1 RUST_LOG=info \
  /data/cache/target/release/damage-probe --damage under 2>&1 | grep damage_buffer
```

`WAYLAND_DEBUG=1` prints every `wl_surface.damage_buffer(x, y, w, h)` the Vulkan WSI sends — the
protocol-level proof that the rects reach the compositor, independent of what it does with them.

### Windows (the Dell, over ssh)

Ship the tree (`tar` excluding `target/` and the Android build output), `cargo build --release` with
`CARGO_TARGET_DIR=C:\dev\target-probe`, then run it **inside the interactive session** — an ssh shell has
no desktop. The proven recipe is the `frust-gate-shot` scheduled task (InteractiveToken) pointed at a
`step.ps1` that `Start-Process`es the exe with `WGPU_BACKEND` set, waits 7 s, `CopyFromScreen`s a PNG,
kills it, for each (backend, mode). Set `RUST_LOG=info` and redirect stderr to capture the adapter lines.

### Android (NativeActivity host in `android/`)

```bash
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.2.13676358
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release --lib
(cd android && echo "sdk.dir=$HOME/Android/Sdk" > local.properties && ./gradlew --no-daemon -q assembleDebug)
adb install -r -g android/app/build/outputs/apk/debug/app-debug.apk
adb shell setprop debug.damage_probe.mode under        # none | exact | under
adb shell setprop debug.damage_probe.backend vulkan    # or gl (the EGL path); '' = wgpu's default
adb shell am start -n dev.frust.damageprobe/android.app.NativeActivity
adb logcat -s 'damage-probe:*'
adb exec-out screencap -p > under.png
```

The other properties: `debug.damage_probe.full_every`, `.step`, `.size`, `.fps`. **`screencap` cannot
show a trail**: it re-composites the layer from the app's buffer, which is always complete. The Android
rows therefore record whether the rects are accepted by the platform swapchain (extension advertised,
present succeeds) and what SurfaceFlinger exposes, not a visual proof — see RESULTS.md.

## Recording a row

One row per (machine, adapter + driver, backend, display server / compositor) in `RESULTS.md`: the date,
the wgpu rev, the `none` / `exact` / `under` outcome, the protocol evidence (`damage_buffer` lines,
logcat, logs), the verdict `honoured` / `ignored` / `delivered, use unverified` / `skipped: <reason>`,
and the evidence image under `evidence/` (cropped to the probe window, kept small).
