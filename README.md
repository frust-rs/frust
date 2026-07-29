<p align="center">
  <img src="docs/assets/branding/banner-wide.png" alt="Frust" width="100%">
</p>

<h1 align="center">Frust</h1>
<p align="center"><em>Fearless UI in Rust</em></p>
<p align="center"><a href="https://frust.dev">frust.dev</a></p>

## What is Frust?

Frust is a Rust UI framework: a declarative `View` API over a retained
widget tree, rendered through a renderer-agnostic vector scene into a GPU
backend (Vello/wgpu), with first-class Android, iOS, and desktop shells —
plus a `frust` CLI that scaffolds, builds, and drives apps on all three
platforms, the way the Flutter CLI does for Flutter.

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
> - **iOS Simulator cannot render** under the pinned Vello 0.9 / wgpu 29
>   (the Simulator GPU lacks `INDIRECT_EXECUTION`); physical iOS devices
>   are unaffected — see `docs/DEVELOPMENT.md` Known Issues.
> - **Back handling is single-navigator, process-wide** — concurrently-live
>   navigators (per-tab stacks, multi-window) are unsupported until the
>   back provider slot is widened to a stack.
>
> Frust also inherits Vello 0.9's alpha-state limitations, notably:
>
> - [Blur and filter effects are unimplemented](https://github.com/linebender/vello/issues/476)
>   (glass materials degrade to opaque fills).
> - [Conflation artifacts](https://github.com/linebender/vello/issues/49).
> - [GPU memory allocation strategy](https://github.com/linebender/vello/issues/366).
> - [Glyph caching](https://github.com/linebender/vello/issues/204).
> - [Bitmap color-emoji strikes that aren't RGBA8 are skipped](https://github.com/linebender/vello/issues/1031)
>   (Android CBDT emoji at risk; desktop sbix verified safe).

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
| `frust-render` | The wgpu + Vello GPU backend that encodes a scene and presents it to a window surface. |
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
Vello/wgpu encode-present path, text shaping, the mobile frame gate, and
the measurement tooling. Every chapter anchors to real files in this repo
and ends with runnable experiments (build a scene by hand, break the S1
bubble-chart benchmark scenario on purpose, trace a frame with
`FRUST_TRACE=1`), plus a
verified watchlist of talks and university lectures for the theory —
after you've seen the mechanism working.

## License

Licensed under MIT OR Apache-2.0.
