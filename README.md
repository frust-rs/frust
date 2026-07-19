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

## License

Licensed under MIT OR Apache-2.0.
