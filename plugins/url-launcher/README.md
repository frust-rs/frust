# frust-url-launcher

A minimal **external URL launcher** plugin for frust apps — Android
`Intent(ACTION_VIEW)`/`startActivity` over plain JNI, iOS
`UIApplication.openURL(_:options:completionHandler:)` via `objc2-ui-kit`,
desktop (macOS/Linux/Windows) the platform's own opener (`open`/`xdg-open`/
`ShellExecuteW`).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-url-launcher = { path = "<frust>/plugins/url-launcher" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

```rust
use frust_url_launcher::UrlLauncher;

// Fire-and-forget: ignore the Result in the common case (there is nothing
// actionable to do differently on this platform once a launch is dispatched).
let _ = UrlLauncher::open_external("https://example.com/pricing");
```

No manifest permission, no Info.plist key, no Gradle module — the `frust`
TUI's **Add Plugin** dialog only adds the Cargo dependency above for this
plugin.

---

## 2. The validator's rule table

Every call to [`open_external`] runs the same validator before touching any
platform API, regardless of target. A URL is rejected unless:

| Rule | Closes |
|---|---|
| Non-empty, at most 8192 bytes | pathological/empty input |
| ASCII only, no control byte, no literal whitespace (a `%20` escape is fine) | raw IRIs, injected control sequences |
| Scheme (before the first `:`) is `http`/`https`, case-insensitively, followed by `//` | `javascript:`, `intent:`, `tel:`, `file:`, and similar deep-link/script-injection schemes |
| Authority (up to the first `/`, `?` or `#`) is non-empty and contains no `@` | userinfo phishing (`https://trusted.example@evil.test/`) |
| Every byte is RFC 3986 `unreserved` (`A-Za-z0-9-._~`), `reserved` (`` :/?#[]@!$&'()*+,;= ``), or a valid `%XX` escape | malformed/ambiguous percent-encoding |

A rejected URL never reaches a backend — [`UrlLauncher::open_external`]
returns `Err(UrlLauncherError::InvalidUrl)` before any platform call.

---

## 3. Platform caveats

- **Android sets `FLAG_ACTIVITY_NEW_TASK` unconditionally.** The host shell
  hands this plugin the process-lifetime *application* `Context`, not an
  `Activity` one, and `Context.startActivity` on a non-`Activity` context
  throws without that flag. No `<queries>` manifest entry, no
  `resolveActivity` probe: this backend just calls `startActivity` and maps
  the resulting `ActivityNotFoundException` (if nothing can handle the
  intent) to `UrlLauncherError::NoHandler`.
- **iOS dispatches asynchronously to the main thread and never reports a
  post-dispatch failure.** `UIApplication` is UIKit's `MainThreadOnly`;
  `open_external` returns `Ok(())` immediately after handing the
  lookup-and-open sequence to `dispatch_get_main_queue()` — a background
  caller is never blocked, but a subsequent "no handler" or "app suspended"
  failure on that platform has nothing left to report back to. This crate
  does not call `canOpenURL:` (it would need an `Info.plist`
  `LSApplicationQueriesSchemes` entry for no discriminating value, since
  every URL here is already `http`/`https`).
- **Desktop has no deep-link callback.** `open`/`xdg-open`/`ShellExecuteW`
  hand the URL to the OS and return; there is no way for the launched
  browser to hand a result (an OAuth authorization code, say) back to this
  process. An app that needs a full OAuth round trip on desktop needs a
  typed code delivered through some other channel (a local loopback
  listener, a manually pasted code, …) — this plugin only opens the URL.
- **Windows uses `ShellExecuteW`, not `cmd /C start`.** `cmd.exe`'s
  command-line parser treats `&` as a command separator and expands `%VAR%`
  references, both of which a legitimate URL can contain; `ShellExecuteW`
  resolves the registered protocol handler directly, with no shell parsing
  in between.

---

## 4. Fire-and-forget contract

`UrlLauncher::open_external` returns a `Result<(), UrlLauncherError>` for the
caller that wants to distinguish *why* nothing opened (an invalid URL, an old
Android scaffold predating `nativeInitPlatform`, no installed handler, a
genuine backend failure) — but the overwhelmingly common call site (a "view
in browser" button, an in-app link) ignores it entirely (`let _ =
UrlLauncher::open_external(...)`). None of `UrlLauncherError`'s `Display`
messages echo the URL back, so logging the error directly never leaks the
target origin.

---

## 5. Caveats

- **This plugin only opens `http`/`https` URLs.** The validator rejects
  every other scheme outright (see §2) — there is no escape hatch for a
  custom URL scheme or deep link in v1.
- **`frust create --overwrite` is a non-issue here.** There is no manifest,
  plist, or Gradle mutation for Add Plugin to re-apply — the Cargo
  dependency line is the entire integration surface.

## 6. Device gate

### Android — PASSED, 2026-09-21

Device: Xiaomi 12 (`2201123G`, codename `cupid`), LineageOS, Android 16
(`ro.build.version.sdk` = 36), connected over network `adb`. App: the
`examples/playground` debug APK (`applicationId` `it.f0x.playground`) built
with `frust build apk --debug` against SDK platform `android-36`,
build-tools `36.0.0`, NDK `28.2.13676358`. Device default `http`/`https`
handler: `org.lineageos.jelly`.

**Merged manifest** — `aapt2 dump xmltree --file AndroidManifest.xml
app-debug.apk`, confirming the `frustplay` filter survives manifest merging
(not just the source file):

```
E: intent-filter (line=59)
    E: action (line=60)
      A: android:name="android.intent.action.VIEW"
    E: category (line=62)
      A: android:name="android.intent.category.DEFAULT"
    E: category (line=63)
      A: android:name="android.intent.category.BROWSABLE"
    E: data (line=65)
      A: android:scheme="frustplay"
...
A: package="it.f0x.playground"
A: android:launchMode(0x0101001d)=1
```

**Browser-out leg** — tapped `Open example.com` on the playground's
`URL launcher` page. `adb logcat -v time ActivityTaskManager:I *:S`:

```
09-21 22:50:40.216 I/ActivityTaskManager( 2253): START u0 {act=android.intent.action.VIEW dat=https://example.com/... flg=0x10000000 xflg=0x4 cmp=org.lineageos.jelly/.MainActivity} with LAUNCH_SINGLE_TASK from uid 10587 (it.f0x.playground) (BAL_ALLOW_VISIBLE_WINDOW) result code=0
```

`flg=0x10000000` is `FLAG_ACTIVITY_NEW_TASK` — the flag this backend must
set because the shell hands out the *application* `Context`. The browser
foregrounded on the Example Domain page (screenshot taken), and
`dumpsys activity activities` reported:

```
topResumedActivity=ActivityRecord{40948244 u0 org.lineageos.jelly/.MainActivity t43}
```

On-page status line, verbatim:

```
status: open_external("https://example.com") -> Ok(())
```

**Deep-link-back leg** — `adb shell am start -a android.intent.action.VIEW
-d 'frustplay://back?ok=1' it.f0x.playground`:

```
Starting: Intent { act=android.intent.action.VIEW dat=frustplay://back/... pkg=it.f0x.playground }
Warning: Activity not started, its current task has been brought to the front
topResumedActivity=ActivityRecord{142060989 u0 it.f0x.playground/.MainActivity t42}
```

The app returned to the foreground and the page's deep-link label
repainted, verbatim:

```
latest: frustplay://back?ok=1
```

(The `singleTop` launch mode is why this is a `onNewIntent` delivery into
the running task rather than a fresh activity — the "Activity not started"
warning above is the expected shape, not a failure.)

**Negative control** — tapped `Open invalid (javascript:)`. On-page status,
verbatim:

```
status: open_external("javascript:alert(1)") -> Err(InvalidUrl)
```

`ActivityTaskManager` logged **zero** `START` lines for that tap and the
foreground activity stayed `it.f0x.playground/.MainActivity`, confirming the
Rust validator rejects the URL *before* any `Intent` is constructed rather
than relying on the OS to refuse it.

### iOS — browser-out and validator legs PASSED, 2026-09-22; return leg not yet run

Run by the maintainer on a physical iPhone from a macOS session, building
the `examples/playground` iOS app. Device model and iOS version were not
recorded.

- **Browser-out leg — PASSED.** Tapping `Open example.com` opened the
  browser on `example.com`, so the main-queue dispatch and the
  `openURL:options:completionHandler:` call deliver on real hardware.
- **Negative control — PASSED.** Tapping `Open invalid (javascript:)`
  showed `Err(InvalidUrl)`, so the Rust validator rejects before the
  platform call on this backend too.
- **`src/apple.rs` compiles for a real iOS target.** This is implied rather
  than separately measured: the playground app cannot build and run on a
  device without compiling `frust-url-launcher`'s iOS backend, and the page
  that drove the test calls `UrlLauncher::open_external` directly. This
  settles the crate's `objc2-ui-kit` feature closure in practice — the
  declared `["std", "UIResponder", "UIApplication", "block2"]` set is
  sufficient. A standalone
  `cargo clippy --target aarch64-apple-ios-sim -p frust-url-launcher
  --all-targets -- -D warnings` has still not been recorded, so the
  crate's lint-clean status on that target remains unproven.
- **Deep-link-back leg — NOT YET RUN.** The `frustplay://back?ok=1` return
  into `frust::deep_links().latest` has not been exercised on iOS. The
  Android run covers the equivalent path, but the iOS `frust_on_deep_link`
  route is a different implementation and is not proven by it.

### Desktop — partially covered

Linux/macOS spawn-and-reap behaviour is covered by this crate's own unit
tests (including a fake-`PATH` `NoHandler` case and a successful-spawn
case). The Windows `ShellExecuteW` arm has never been compiled or run —
see `url-launcher-windows-leg-unrun` in `docs/LIMITATIONS.md`.

[`open_external`]: https://docs.rs/frust-url-launcher/latest/frust_url_launcher/struct.UrlLauncher.html#method.open_external
