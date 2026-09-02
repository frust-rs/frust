<p align="center">
  <img src="docs/assets/branding/banner-wide.png" alt="Frust" width="100%">
</p>

<h1 align="center">Frust</h1>
<p align="center"><em>Fearless UI in Rust</em></p>
<p align="center"><a href="https://frust.dev">frust.dev</a></p>

## What is Frust?

Frust is a Rust UI framework: a declarative `View` API over a retained
widget tree, rendered through a renderer-agnostic vector scene into a GPU
backend (the frust-owned `frust-engine` strips-on-wgpu renderer), with
first-class Android, iOS, and desktop shells — plus a `frust` CLI that
scaffolds, builds, and drives apps on all three platforms, the way the
Flutter CLI does for Flutter.

## Status

Pre-1.0. APIs are unstable and may change without notice between commits.

> [!WARNING]
> Frust can currently be considered in an alpha state. In particular, we're
> still working on the following:
>
> - [**Animation performance on mobile**](https://github.com/f0x-it-llc/frust/issues/3)
>   — a perpetual animation anywhere on screen currently drives a full-scene
>   re-encode and a full-surface GPU pass at the display's max refresh rate
>   (device heat, janky scroll on animated screens). Root-caused with a fix
>   plan in flight; see the issue for measurements and details.
> - **No partial repaint / damage regions** — a whole-stack gap shared with
>   the wider ecosystem today ([wgpu#2869](https://github.com/gfx-rs/wgpu/issues/2869),
>   [xilem#789](https://github.com/linebender/xilem/issues/789)); every
>   frame re-rasterizes the full surface.
> - **Back handling is single-navigator, process-wide** — concurrently-live
>   navigators (per-tab stacks, multi-window) are unsupported until the
>   back provider slot is widened to a stack.
>
> `frust-engine`, the frust-owned strips-on-wgpu renderer, carries its own
> named alpha-state gaps, notably:
>
> - Bitmap-strike color emoji (PNG/BGRA/Mask — e.g. Android's CBDT strikes)
>   do not render; COLR-format color emoji do.
> - A per-corner blurred shadow/glow rounds to its largest corner rather
>   than each corner independently.
> - The glyph atlas is built on a cache upstream itself labels
>   experimental, wrapped in frust-owned policy.
>
> See `docs/LIMITATIONS.md` for the full, evidenced register.

## Quickstart

Prerequisites: Rust 1.88+ (edition 2024); Android needs the
`aarch64-linux-android` target, `cargo-ndk`, and an Android SDK/NDK; iOS
(macOS host only) needs Xcode and the `aarch64-apple-ios-sim`/
`aarch64-apple-ios` targets. See `docs/DEVELOPMENT.md` for the full list.

```bash
# Scaffold a new app
frust create my_app
cd my_app

# Build and run on a connected device/emulator/simulator
frust run

# Or preview on desktop
cargo run
```

`frust build`/`frust doctor` round out the CLI — see `docs/DEVELOPMENT.md`
for the full command surface.

## Workspace crates

| Crate | Responsibility |
|---|---|
| `frust-core` | The declarative `View` trait, retained `Widget` trait, layout, event/focus/capture pass, animation vocabulary, and pull-based accessibility (semantics) pass. |
| `frust-scene` | The renderer-agnostic vector scene / display list — the stable seam between widgets and the GPU backend. |
| `frust-render` | The wgpu GPU backend, driving the frust-owned `frust-engine` strip renderer, that presents a scene to a window surface. |
| `frust-text` | Text shaping (Parley/Fontique/HarfRust/Swash) and the `TextEditor` engine platform IME bridges drive. |
| `frust-theme` | Design tokens: Material 3 and Cupertino baselines, color/type/shape/elevation/motion/glass scales. |
| `frust-widgets` | The baseline + Material + Cupertino widget catalog, plus navigation (imperative navigator, declarative router, shared-element transitions). |
| `frust-reactive` | The leaf reactive substrate (signals, background runtime, deep-link and back-press sources) shells and app code build on. |
| `frust-shell-desktop` | Desktop preview shell (winit) for `cargo run`-based development. |
| `frust-shell-android` | Android platform shell behind the JNI surface a generated app's Kotlin `SurfaceView` calls into. |
| `frust-shell-ios` | iOS platform shell behind the C-ABI surface a generated app's Swift code calls into. |
| `frust-shell-common` | Platform-agnostic shell plumbing shared by the Android and iOS shells. |
| `frust` | The public app-author facade: `Component`/`app!`/`run` and the widget vocabulary apps are written against. |
| `frust-cli` | The standalone `frust` binary: project scaffolding, environment doctor, device discovery, and the `run`/`build`/`clean` pipelines. |

## Learning the rendering pipeline

[`docs/learning/`](docs/learning/README.md) is a hands-on, lab-based
curriculum for understanding how Frust turns a `View` into pixels — from
the display list through the widget paint seam, the frame loop, the
frust-engine/wgpu encode-present path, text shaping, the mobile frame gate, and
the measurement tooling. Every chapter anchors to real files in this repo
and ends with runnable experiments (build a scene by hand, break the S1
bubble-chart benchmark scenario on purpose, trace a frame with
`FRUST_TRACE=1`), plus a
verified watchlist of talks and university lectures for the theory —
after you've seen the mechanism working.

## License

Licensed under MIT OR Apache-2.0.
