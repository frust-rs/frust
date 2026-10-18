# damage-probe results

wgpu rev `167e7687` = gfx-rs/wgpu#10152 head (sauliusvl:damage-present-regions, 2026-10-17). Method and
build recipes: README.md. Evidence images: `evidence/` (probe window crops; raw screenshots and logs are
attached to the Zabin cards).

Verdict vocabulary: **honoured** = under-reporting leaves a visible trail (the compositor consumed the
rects); **ignored** = clean in every mode and the backend/driver drops the rects; **delivered, use
unverified** = the rects demonstrably reach the platform (protocol log / extension enabled, present
succeeds) but the display path showed no observable difference; **skipped** = leg not run, with the reason.

## Matrix (2026-10-18/19)

| Date | wgpu rev | Machine | Adapter / driver | Backend | Display server / compositor | none | exact | under | Evidence | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|
| 2026-10-18 | 167e7687 | Rig (precision), linux-native container | NVIDIA T400 4GB, 610.57.04 | Vulkan | Xorg `:20` + kwin_x11 (X Present) | clean | clean | **trail** | `evidence/t400-x11-kwin-under.png` vs `-exact.png`; log: `VK_KHR_incremental_present: supported`, Bgra8Unorm 800x600 Fifo, ~55 fps | **honoured** |
| 2026-10-18 | 167e7687 | Rig, nested compositor on `:20` | NVIDIA T400 4GB, 610.57.04 | Vulkan | kwin_wayland 6.7.4 nested (`--x11-display :20`) | clean | clean | clean | `WAYLAND_DEBUG`: 459 `wl_surface.damage_buffer(x, y, 48, 48)` lines in `under`, two per frame in `exact` (`damage_buffer(540,168,48,48)` + `(546,168,48,48)`); `evidence/t400-nested-kwin-wayland-under.png` | **delivered, use unverified** — the nested X11 backend repaints the whole output every frame, so no trail can show; a real DRM session is needed for the compositor-side question |
| 2026-10-18 | 167e7687 | Rig | Intel UHD 770, Mesa 26.2.2 | Vulkan | Wayland session | — | — | — | `vulkaninfo` advertises `VK_KHR_incremental_present` rev 2 | **skipped: no display attached to the iGPU and no Wayland session on the rig** |
| 2026-10-18 | 167e7687 | Dell mini PC (Win11 26200.9711, RDP session) | Intel UHD Graphics 730, 32.0.101.7088 | DX12 | DWM (FLIP_DISCARD swapchain) | clean | clean | clean | `evidence/dell-uhd730-dx12-under.png`; Bgra8Unorm 800x600 Fifo, ~31 fps, no error | **ignored** (by design: the PR drops rects on DX12) |
| 2026-10-18 | 167e7687 | Dell mini PC | Intel UHD Graphics 730, Intel Vulkan 101.7088 | Vulkan (`WGPU_BACKEND=vulkan`) | DWM | clean | clean | clean | log: `VK_KHR_incremental_present: NOT supported by the adapter — rects will be dropped`; present succeeds, ~31-37 fps; `evidence/dell-uhd730-vulkan-under.png` | **ignored** (the Intel Windows Vulkan driver does not advertise the extension; wgpu-core degrades to a full present, no error) |
| 2026-10-18 | 167e7687 | OnePlus 9 (LE2115, Android 15) | Adreno 660, Qualcomm Vulkan a1cf8ab5e3 | Vulkan | SurfaceFlinger | clean | clean | clean (`screencap`) | `VK_KHR_incremental_present: supported`; Rgba8Unorm 1080x2400 Fifo alpha Inherit; ~117 fps; `evidence/oneplus9-vulkan-under-screencap.png` | **delivered, use unverified** — the platform swapchain accepts the regions (it is the one that advertises the extension); `screencap` re-composites from the full buffer, `dumpsys SurfaceFlinger` and a Perfetto `android.surfaceflinger.layers` trace (TRACE_FLAG_EXTRA + BUFFERS) expose no damage region; whether SF/HWC do less work needs a power/GPU measurement (Phase 1) |
| 2026-10-18 | 167e7687 | OnePlus 9 | Adreno 660, OpenGL ES 3.2 V@0530.53 | GLES / EGL (`debug.damage_probe.backend gl`) | SurfaceFlinger | clean | — | clean | the EGL `eglSwapBuffersWithDamage` path the PR author had not run on hardware: present succeeds, no error (after the shader change below) | **delivered, use unverified** |
| 2026-10-18 | 167e7687 | Pixel 5 (redfin, Android 14) | Adreno 620, Qualcomm Vulkan 4783c89 | Vulkan | SurfaceFlinger | clean | — | clean | `VK_KHR_incremental_present: supported`; Rgba8Unorm 1080x2204 Fifo | **delivered, use unverified** |
| 2026-10-18 | 167e7687 | Pixel 5 | Adreno 620, OpenGL ES 3.2 V@0490.0 | GLES / EGL | SurfaceFlinger | clean | — | clean | present succeeds, no error | **delivered, use unverified** |
| 2026-10-18 | 167e7687 | Pixel 4a (sunfish, Android 16) | Adreno 618, Qualcomm Vulkan 85da404 | Vulkan | SurfaceFlinger | clean | — | clean | `VK_KHR_incremental_present: supported`; Rgba8Unorm 1080x2340 Fifo | **delivered, use unverified** |
| 2026-10-18 | 167e7687 | Pixel 4a | Adreno 618, OpenGL ES 3.2 V@0490.0 | GLES / EGL | SurfaceFlinger | clean | — | clean | present succeeds, no error | **delivered, use unverified** |
| 2026-10-18 | 167e7687 | Mac | Apple GPU | Metal | WindowServer | — | — | — | the PR ignores the rects on Metal | **skipped: Ed's leg** (expected: ignored) |

## Findings beyond the matrix

- **No crash, validation error or wrong frame on any backend**, including the two that drop the rects
  (DX12 and a Vulkan driver without the extension) and the three Android GLES/EGL runs.
- **Adreno 660 GLES only**: the first shader (dynamic `u.rects[instance_index]` indexing of a uniform
  array) failed naga's GLSL-ES translation with an **empty error message** (`Shader translation error for
  stage ShaderStages { VERTEX | FRAGMENT }:` then nothing; `wgpu_core::device::resource`). The Pixel 4a and
  Pixel 5 (older Adreno driver line V@0490.0) compiled the same shader. Rewritten with constant indices
  (`select(u.rects[1], u.rects[0], ii == 0u)`) it runs on all three. Unrelated to damage; worth an
  upstream naga report with the shader.
- **The visible-trail method works** where the compositor tracks damage (X Present + kwin_x11 on NVIDIA):
  the `under` trail is exactly the set of positions the probe never reported. `exact` is pixel-clean on
  the same path, so a correct hint costs nothing visually.
- **Android has no observable surface for the hint** with shell-level tools: the layers trace's layer
  block carries the 1080x2400 bounds in both `under` and `none` and no 48 px rect. Treat Android as
  "accepted by the swapchain"; the compositor-side benefit is a measurement, not a capture.
- **Windows Vulkan is not a free arm**: the Intel UHD 730 Windows Vulkan driver (101.7088) does not
  advertise `VK_KHR_incremental_present`, so on this rig Windows has no honouring backend at all.
