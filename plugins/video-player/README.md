# frust-video-player

One Rust video-playback API for frust apps — open a file, a bundled asset or a
URL; play/pause/seek/rate/volume/loop it; read its state; and host its picture
in a native platform-view slot — over the platform's own player on each OS:
Media3 ExoPlayer on Android (`dev.frust.videoplayer.FrustVideoPlayerHost`,
driven over this plugin's own JNI surface) and AVFoundation's `AVPlayer` on
iOS **and** macOS (driven straight from Rust via `objc2-av-foundation`, no
Swift glue anywhere).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` — the `frust` facade does not re-export it.

> **Templates stay clean.** A generated frust project ships **no** video code,
> permission or plist key. You add exactly the lines below by hand — or let the
> frust TUI's **Add Plugin** dialog apply them for you (§6). On Android the
> plugin's platform code ships as its own Gradle library module rather than as
> files copied into your app. On Apple there is nothing to add at all.

**Platform support:** Android (Media3 ExoPlayer), iOS and macOS (AVFoundation). On every other target `VideoPlayer::open` fails with `VideoError::NotSupported`.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

---

## 1. Add the dependency (always)

```toml
# app Cargo.toml — [dependencies]
frust-video-player = "0.5"
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

```rust
use frust_video_player::{PlayerOptions, VideoPlayer, VideoSource};

let session = VideoPlayer::open(
    VideoSource::Url("https://example.com/clip.mp4".to_owned()),
    PlayerOptions { autoplay: true, ..PlayerOptions::default() },
)?;
```

`open` returns **immediately** — it hands the source to the platform player
and does not wait for it to load. The session starts in
`PlaybackState::Loading`; readiness, the first frame's geometry and any load
failure arrive afterwards (§3).

The app-facing half (`frust_video_player::api`) sits behind the default-on
`frust-api` feature. Building with `--no-default-features` drops the `frust`
facade dependency entirely, which is what keeps this crate's platform-plugin
charter line mechanically checkable.

---

## 2. The video surface is a platform view

This crate never paints a frame itself. The picture is a **native view the OS
composites**, reserved through `frust::platform_view`, so the only extra setup
is whatever that native side needs on each platform.

With the `frust-api` feature (the default) you do not wire that by hand:

```rust
use frust_video_player::api::{use_video_player, video_view, VideoPlayerHandle};
use frust_video_player::VideoFit;

// in a Component's init: registers the listener and an on_cleanup for it
let handle: VideoPlayerHandle = use_video_player(session);

// in build: the slot itself, filling its container unless you .size(..) it
video_view(&handle, VideoFit::Contain).size(320.0, 180.0)
```

Without the feature, do the same two calls yourself — never hardcode either
string, they are target-gated:

```rust
frust::platform_view(session.view_type())
    .params_json(session.params_json(VideoFit::Contain))
```

### 2a. Android setup — include this plugin's Gradle module

The TUI Add Plugin dialog makes both edits below for you, idempotently. By
hand:

1. `android/settings.gradle.kts` — include the module by path, and redirect its
   build directory under your app:

   ```kotlin
   include(":frust-video-player")
   project(":frust-video-player").projectDir = frustLocalDir("frust.plugin.frust-video-player.dir")

   gradle.lifecycle.beforeProject {
       if (path == ":frust-video-player") {
           layout.buildDirectory.set(rootDir.resolve("../build/android/frust-video-player"))
       }
   }
   ```

2. `android/app/build.gradle.kts` — depend on it:

   ```kotlin
   dependencies {
       implementation(project(":frust-video-player"))
   }
   ```

The module's own `AndroidManifest.xml` declares `android.permission.INTERNET`
and a no-op init `ContentProvider`; the manifest merger folds both into your
app, so there is nothing to add to your own manifest and the permission cannot
outlive the plugin. Its R8 keep rules travel with it too
(`consumer-rules.pro`), so a minified release build needs no hand-edited
`proguard-rules.pro`. `dev.frust.videoplayer.FrustVideoPlayerHost` and
`dev.frust.videoplayer.VideoPlayerViewFactory` are hard contracts (JNI symbol
names and a classloader-resolved `viewType`) — never move or rename either.
The module compiles against `compileSdk = 36`, a hard floor Media3 1.11.0's
own metadata imposes.

### 2b. Apple setup — nothing beyond §1's Cargo dependency

iOS and macOS need **no** platform addition: no Swift package reference, no
Xcode project edit, no `Info.plist` key. Both native views are written in Rust
with `objc2`'s `define_class!` — a `UIView` hosting an `AVPlayerLayer` on iOS,
an `NSView` whose *backing* layer is the `AVPlayerLayer` on macOS — and each
registers its factory the first time you `open` a session. Nothing in this
crate is exported over the C ABI.

- **iOS** resolves the factory by its Objective-C runtime name
  (`NSClassFromString`), the bare `VideoPlayerViewFactory` — the same
  zero-Swift shape `frust-native-widgets` uses.
- **macOS** registers a `DesktopViewFactory` into `frust-plugin`'s desktop
  registry under that same name; the desktop shell resolves it from there and
  parents the returned `NSView` above the window's content view. See §5 for
  what that costs you.

A remote `http://` URL additionally needs your own
`NSAppTransportSecurity` exception — App Transport Security refuses plain
`http` by default. `https` needs nothing.

---

## 3. Threading & events

**Nothing in this crate blocks, and nothing is UI-thread-guarded.** Unlike
`frust-camera`, there is no `spawn_blocking` pairing to remember: `open` and
every control method (`play`, `pause`, `seek_to`, `set_rate`, `set_volume`,
`set_looping`) post a command at the platform player and return. They report
only whether the command was *accepted*, never its outcome, and they are safe
to call straight from an event handler.

Outcomes come back the other way:

- `PlayerSession::snapshot()` is a lock-free read of the current state,
  position, duration, decoded video size and error — callable from any thread,
  which is what a `build` uses.
- `PlayerSession::set_listener(..)` registers **one** listener (a second call
  replaces the first) that receives `PlayerEvent`s after the snapshot has
  already been updated. Dropping the returned `ListenerHandle` unregisters it.

The listener runs on the **platform main thread**. Two rules bind it:

1. **Never block** — it is holding the thread the whole UI runs on.
2. **Never call back into the session** — a control call from inside a
   delivery is refused with `VideoError::Reentrant` rather than re-entering
   the backend. (`close` is the one exception: closing a failed session from
   its own error callback is allowed.) Write a signal and return.

`VideoPlayerHandle` (the `frust-api` half) is exactly that pattern already
written: it owns the session, registers the one listener, and exposes five
`RwSignal`s a `build` reads — `state`, `position`, `duration`, `video_size`,
`error` — plus pass-through `play`/`pause`/`seek_to`/`set_rate`/`set_volume`/
`set_looping`/`close` and `session()` for anything it has no pass-through for.
Prefer `use_video_player(session)` over `VideoPlayerHandle::new(session)` from
inside a component's `init`: it additionally registers an `on_cleanup` that
unregisters the listener at teardown.

---

## 4. Sources & formats

```rust
VideoSource::File(PathBuf::from("/data/.../clip.mp4"))  // device filesystem
VideoSource::Asset("clips/intro.mp4".to_owned())        // in-app-bundle resource
VideoSource::Url("https://example.com/clip.mp4".to_owned())
VideoSource::Url("https://example.com/stream.m3u8".to_owned())  // HLS
```

`Asset` is **not** a filesystem path: on Android it is a path under the
module's `assets/` directory, on Apple a main-bundle resource path. An
unbundled macOS `cargo run` binary has no main bundle to resolve one against —
use a `File` path there.

`Url` accepts `http` and `https` only, on every backend.

Container and codec support is the **platform player's**, and it is not
symmetric. This crate forwards the source and reports what the player says, so
treat a `VideoError::Decoder` as a per-platform answer:

| Source | Android (ExoPlayer) | iOS / macOS (AVFoundation) |
|---|---|---|
| MP4 / H.264+AAC | yes | yes — the intersection to ship |
| HLS (`.m3u8`) | yes, via the `media3-exoplayer-hls` artifact this module already declares | yes, natively |
| Matroska / WebM (VP8/VP9) | yes | **no** — reports `VideoError::Decoder` (`video-mkv-webm-apple-unsupported` in `docs/LIMITATIONS.md`) |
| anything else | whatever that player's extractor set covers | whatever AVFoundation covers |

On a target with no backend at all — desktop Windows and Linux, and wasm —
`open` fails soft with `VideoError::NotSupported` (never a panic) and
`VIEW_TYPE` is the empty string, reserving no slot. See
`video-web-windows-linux-unavailable-v1` in `docs/LIMITATIONS.md`.

---

## 5. Lifecycle & caveats

- **No automatic background pause, on any platform.** Playback keeps running
  when your app backgrounds — audio-only continuation is a legitimate choice
  and the plugin cannot tell it from an app that wants a hard pause. Drive
  `pause`/`play` from your own lifecycle handling if you want otherwise
  (`video-no-auto-pause-in-background` in `docs/LIMITATIONS.md`).
- **The slot and the session have independent lifetimes.** Scrolling the
  picture off-screen and back reattaches to the *same* player rather than
  reopening it. Only `close()` — or dropping the session, which runs the same
  thing — releases the platform player. After that every control call reports
  `VideoError::Closed` and the snapshot reads `PlaybackState::Idle`.
- **iOS audio session.** The plugin configures the process's shared
  `AVAudioSession` for playback, so a video is audible with the ringer switch
  silenced. `PlayerOptions::mix_with_others` adds `.mixWithOthers` so playback
  ducks alongside another app's audio instead of interrupting it. It is
  iOS-only — macOS has no `AVAudioSession`, and the option is ignored rather
  than emulated there.
- **macOS is Mode A: put controls beside the picture, never on top of it.**
  The hosted `NSView` is an opaque native sibling composited *above* the frust
  surface, so anything a `Stack` places over the slot is physically hidden
  underneath it, and every pointer/wheel event inside the slot is consumed by
  the native view — a scroll over the video does not scroll the page. The
  hosted view also reaches new geometry up to one frame ahead of the frust
  content it is pinned to, visible while a scroll or resize animates. See
  `desktop-platform-view-mode-a-only` and
  `desktop-platform-view-frame-lead` in `docs/LIMITATIONS.md`. Overlaying
  chrome inside a `Stack` is a **mobile-only** affordance, and even there
  `frust::platform_view`'s own Z-shields contract applies.
- **`VideoFit::Cover` on Android is a device-gate item.** `Contain`
  letterboxes and is contained by construction; `Cover` sizes the picture to
  overflow its slot and relies on being clipped, which a `SurfaceView`'s
  compositor layer was observed not to honour in frust's window arrangement.
  Prefer `Contain` until that is confirmed on hardware.
- **Media3 versions move in lockstep.** `media3-exoplayer` and
  `media3-exoplayer-hls` are both pinned to `1.11.0` exactly; mixing two Media3
  versions in one app is unsupported upstream. `media3-ui` is deliberately
  absent — this module hosts a plain `SurfaceView` and frust widgets paint the
  controls.
- **`frust create --overwrite` destroys the Android additions.** `--overwrite`
  re-renders the generated project wholesale, dropping the Gradle include and
  the module dependency. Both are idempotent — re-run the Add Plugin dialog
  (or re-apply §2a). A missing include fails at *runtime* as a blank slot, not
  at build time, since nothing in the Rust build references the Kotlin classes.

---

## 6. The TUI Add Plugin dialog automates all of this

Everything in §1 and §2a — the `Cargo.toml` dependency and the
`:frust-video-player` Gradle include plus its app-module dependency — is
applied for you, idempotently, by the frust TUI's **Add Plugin** dialog
(select `video-player`). There is nothing for it to do on Apple: those two
contributions are the whole integration, because the iOS and macOS factories
are Rust. This README is the manual contract that dialog encodes.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
