<p align="center">
  <img src="docs/assets/branding/banner-wide.png" alt="Frust" width="100%">
</p>

<h1 align="center">Frust</h1>
<p align="center"><em>Fearless UI in Rust</em></p>
<p align="center"><a href="https://frust.dev">frust.dev</a></p>

Frust is a Rust-native, mobile-first declarative UI framework: write one `View` tree in Rust and
ship it to Android, iOS, macOS, Windows, Linux, and the web, rendered everywhere by the
frust-owned `frust-engine` GPU pipeline.

## Documentation

- [frust.dev](https://frust.dev) — docs home
- [Get started](https://frust.dev/docs/get-started)
- [The `frust` TUI](https://frust.dev/docs/tooling/tui)
- [Widget catalog](https://frust.dev/widgets)
- [Architecture](docs/ARCHITECTURE.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Learning the rendering pipeline](docs/learning/README.md)

## About Frust

### Beautiful UIs

Frust apps are built from a declarative `View` API that diffs into a retained `Widget` tree —
no imperative view mutation. Design systems ship as sibling plugin crates built entirely on the
public `frust::authoring` toolkit, the same seam a third-party design system uses:
`frust-glyph`, `frust-material` (Material 3), `frust-cupertino`, `frust-shadcn`, and `frust-beui`.

**Built with Frust: muxr** — a mobile client for remote zellij/herdr terminal sessions, built
entirely with Frust.

<p align="center">
  <img src="docs/assets/readme/muxr-connect.png" alt="muxr: connect to a server" width="180">
  <img src="docs/assets/readme/muxr-sessions.png" alt="muxr: sessions list" width="180">
  <img src="docs/assets/readme/muxr-terminal-htop.png" alt="muxr: terminal running htop" width="180">
  <img src="docs/assets/readme/muxr-panes.png" alt="muxr: pane layout" width="180">
  <img src="docs/assets/readme/muxr-zellij.png" alt="muxr: zellij session" width="180">
</p>

### Fast

`frust-engine`, the frust-owned strips-on-wgpu render engine, is a stateless-per-frame compiler
that walks the renderer-agnostic `frust-scene` display list into sparse strips, packs them into
the GPU layouts its own WGSL reads, and records the frame's passes into a caller-owned
`wgpu::CommandEncoder` — every rendering path returns an error rather than panicking. A tracked
signal write wakes the shell for the next frame through the process-wide `FrameWaker`; with no
write there is no wake, so a static screen renders no frames at all, and a decorative loop is
paced to the active theme's cosmetic rate rather than running every vsync.

### Productive development — the TUI

Bare `frust` opens `frust-tui`, a mouse-first terminal workbench, rather than dropping you at a
bare CLI:

<p align="center">
  <img src="docs/assets/readme/tui-toolchain-wizard.png" alt="frust TUI: toolchain setup wizard" width="31%">
  <img src="docs/assets/readme/tui-run-dialog.png" alt="frust TUI: run configuration dialog" width="31%">
  <img src="docs/assets/readme/tui-devtools-inspector.png" alt="frust TUI: DevTools widget inspector" width="31%">
</p>
<p align="center"><em>Toolchain setup wizard · run configuration · DevTools inspector</em></p>

- A **toolchain setup wizard** checks Rust plus each platform area (Android, iOS, Desktop, Web)
  and offers a guided fix action per gap.
- A **projects list** with new-project scaffolding, and **device discovery** feeding a run dialog
  that launches on one or several devices at once, in debug, profile, or release mode.
- **Concurrent run sessions**, each with its own log tab.
- A **DevTools panel** (Performance, System, Inspector, Network) for a running session.
- **Add plugin**, to wire an OS-capability or design-system plugin into the current project.
- An embedded **MCP server** for AI agents and an embedded **DAP server** for editor debuggers,
  both driving the same sessions the workbench shows.

See [the website's TUI docs](https://frust.dev/docs/tooling/tui) for the full picture. (There is
no hot reload — a code change still needs a rebuild.)

### Extensible

Frust apps are plain Rust in the same process as the OS, so an OS-capability plugin calls
platform APIs directly through FFI (`jni` on Android, `objc2` on Apple) rather than through a
per-plugin Kotlin/Swift bridge: `frust-shared-preferences`, `frust-secure-storage`,
`frust-camera`, `frust-clipboard`, `frust-haptics`, `frust-url-launcher`, `frust-auth-session`,
`frust-iap`, `frust-database`, `frust-i18n`, `frust-video-player`, and `frust-native-widgets`
(real platform controls). The generated app shell itself still carries a thin native entry point
— Kotlin on Android, Swift on iOS — that the Rust core calls into. `frust create --arch
clean-signals` scaffolds an opt-in clean-architecture variant wired to `clean-signals-frust`.

## Status

Pre-1.0. APIs are unstable and may change without notice between commits.

> [!WARNING]
> Frust can currently be considered in an alpha state. In particular, we're
> still working on the following:
>
> - **No partial repaint / damage regions** — every rendered frame
>   re-rasterizes the full surface, a whole-stack gap shared with the wider
>   ecosystem today ([wgpu#2869](https://github.com/gfx-rs/wgpu/issues/2869),
>   [xilem#789](https://github.com/linebender/xilem/issues/789)). The mobile
>   frame gate, cosmetic-loop pacing, and viewport culling bound *how many*
>   frames render (a static screen renders none; a decorative loop is capped
>   at the theme's cosmetic rate), not what each one costs.
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

Prerequisites: Rust 1.88+ (edition 2024). Nothing is published to crates.io yet, so the `frust`
binary is built from a checkout rather than `cargo install`ed by name — see
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for platform toolchains (Android/iOS/Web) beyond the
desktop preview.

```bash
# Clone the framework and build the `frust` CLI from it
git clone https://github.com/f0x-it-llc/frust
cd frust
cargo install --path crates/frust-cli

# Bare `frust` opens the terminal workbench: the toolchain wizard gets this
# machine building Frust apps, then the workbench scaffolds and runs one
frust
```

The workbench's actions map to CLI commands you can also run directly:

```bash
frust create my_app && cd my_app   # scaffold a new app
frust run                          # build/install/launch on a connected device/emulator/simulator
frust doctor                       # what the toolchain wizard checks, non-interactively
```

## Hello, Frust

```rust
// crates/frust/src/lib.rs (doctest)
use frust::{Component, View, AnyView, any, text};

struct Counter;

impl Component for Counter {
    type State = i32;

    fn init(&self) -> i32 {
        0
    }

    fn build(&self, state: &mut i32) -> AnyView<i32> {
        any(text(format!("count: {state}")).size(32.0))
    }
}

frust::run(Counter).unwrap();
```

## Workspace crates

**Framework**

| Crate | Responsibility |
|---|---|
| `frust-core` | The declarative `View` trait, retained `Widget` trait, layout, event/focus/capture pass, animation vocabulary, and pull-based accessibility (semantics) pass. |
| `frust-scene` | The renderer-agnostic vector scene / display list — the stable seam between widgets and the GPU backend. |
| `frust-render` | The wgpu GPU backend, driving the frust-owned `frust-engine` strip renderer, that presents a scene to a window surface. |
| `frust-gpu` | The `wgpu` adapter/device/surface substrate `frust-engine` and `frust-render` build on: pipeline cache, shader loading, resource pool/arena, encoder, headless testing. |
| `frust-engine` | The frust-owned strips-on-wgpu render engine: compiles a `frust-scene` scene into sparse strips and records GPU passes into a caller-owned command encoder. |
| `frust-text` | Text shaping (Parley/Fontique/HarfRust/Swash) and the `TextEditor` engine platform IME bridges drive. |
| `frust-theme` | Design tokens: Material 3 and Cupertino baselines, color/type/shape/elevation/motion/glass scales. |
| `frust-widgets` | The baseline + Material + Cupertino widget catalog, plus navigation (imperative navigator, declarative router, shared-element transitions). |
| `frust-reactive` | The leaf reactive substrate (signals, background runtime, deep-link and back-press sources) shells and app code build on. |
| `frust-paths` | Leaf platform-directory resolution and atomic-write helpers, shared by desktop-facing shells and plugins. |
| `frust-plugin` | Plugin substrate: publishes the Android `(JavaVM, Context)` handle pair a plugin needs to reach the host OS, with zero per-plugin native code. |
| `frust` | The public app-author facade: `Component`/`app!`/`run` and the widget vocabulary apps are written against. |

**Shells**

| Crate | Responsibility |
|---|---|
| `frust-shell-desktop` | Shared winit desktop shell (preview + macOS/Windows/Linux bundles). |
| `frust-shell-macos` | macOS: the native AppKit half of `frust-shell-desktop` (menu bar, app lifecycle). |
| `frust-shell-windows` | Windows: the native Win32 half of `frust-shell-desktop`. |
| `frust-shell-linux` | Linux: the native half of `frust-shell-desktop` (Wayland/X11 app identity). |
| `frust-shell-android` | Android platform shell behind the JNI surface a generated app's Kotlin `SurfaceView` calls into. |
| `frust-shell-ios` | iOS platform shell behind the C-ABI surface a generated app's Swift code calls into. |
| `frust-shell-web` | Browser shell: renders through `frust-engine` on WebGPU with a WebGL2 fallback. |
| `frust-shell-common` | Platform-agnostic shell plumbing shared by every shell above. |

**Tooling**

| Crate | Responsibility |
|---|---|
| `frust-cli` | The standalone `frust` binary: project scaffolding, environment doctor, device discovery, and the `run`/`build`/`clean` pipelines. |
| `frust-drive` | The shared drive logic behind the `frust` CLI — scaffolding, doctor, device discovery, run/build/clean pipelines — consumed by both `frust-cli` and `frust-tui`. |
| `frust-tui` | The mouse-first `frust` terminal workbench described above. |
| `frust-mcp` | An MCP server exposing frust app control and diagnosis to AI agents over Streamable HTTP on `127.0.0.1`. |
| `frust-dap` | An embedded Debug Adapter Protocol server for Frust, hosted exclusively by `frust-tui` — no standalone process. |
| `frust-devtools-protocol` | The dependency-free wire-protocol leaf connecting an in-app Frust debug service to `frust-drive`/`frust-tui`. |
| `frust-devtools` | The in-app debug service a Frust app hosts: widget-tree inspection, frame stats, input injection over a loopback socket. |

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
