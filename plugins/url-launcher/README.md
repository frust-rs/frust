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

### Linux — child-reap and no-handler legs PASSED, 2026-09-22; browser-out leg FAILED (environmental — no live desktop session on this gate host)

Host: Manjaro Linux (rolling, `BUILD_ID=rolling`), kernel `6.18.49-1-MANJARO`
(`x86_64`). `xdg-utils` provides `/usr/bin/xdg-open`, `/usr/bin/xdg-settings`,
`/usr/bin/xdg-mime`. Driven from a tiny scratch binary outside this
repository that calls `frust_url_launcher::UrlLauncher::open_external`
directly and prints the returned `Result` (`frust-url-launcher` depends only
on `thiserror` + `frust-plugin`, so this compiled without `libfontconfig1-dev`
or a full frust app, as expected).

**Desktop environment at gate time: none active.** Before running leg 1 this
was checked and recorded, because it changes what leg 1 can prove:

```
$ echo "XDG_CURRENT_DESKTOP=[$XDG_CURRENT_DESKTOP] XDG_SESSION_TYPE=[$XDG_SESSION_TYPE] DISPLAY=[$DISPLAY] WAYLAND_DISPLAY=[$WAYLAND_DISPLAY]"
XDG_CURRENT_DESKTOP=[] XDG_SESSION_TYPE=[tty] DISPLAY=[] WAYLAND_DISPLAY=[]
$ systemctl is-active sddm
inactive
```

No `Xorg`/`Xwayland`/`kwin`/`gnome-shell`/`mutter`/`sway` process was found
in the process table either. This host's only active sessions at gate time
were remote terminal logins — there was no compositor, no display manager
session, and nothing for a browser to render into. This is a fact about the
gate host at run time, not about the crate.

**Leg 1 — success path — FAILED (environmental).** The default `http`/
`https` handler *is* registered:

```
$ xdg-settings get default-web-browser
zen.desktop
$ xdg-mime query default x-scheme-handler/https
zen.desktop
```

The real call returns `Ok(())`:

```
open_external("https://example.com") -> Ok(())
```

but independent proof that anything actually opened is absent — no opener
or browser process existed a second later:

```
$ ps -eo pid,ppid,stat,cmd | grep -iE "xdg-open|zen-browser|zen-bin|firefox"
(no output)
```

Running the same URL through `xdg-open` directly, with debug tracing on,
explains why:

```
$ XDG_UTILS_DEBUG_LEVEL=2 xdg-open https://example.com
Selected DE generic
/usr/bin/xdg-open: line 1045: www-browser: command not found
/usr/bin/xdg-open: line 1045: links2: command not found
/usr/bin/xdg-open: line 1045: elinks: command not found
/usr/bin/xdg-open: line 1045: links: command not found
/usr/bin/xdg-open: line 1045: lynx: command not found
/usr/bin/xdg-open: line 1045: w3m: command not found
xdg-open: no method available for opening 'https://example.com'
$ echo $?
3
```

With `XDG_CURRENT_DESKTOP` empty, `xdg-open` selects its "generic" fallback
branch, which for an `http`/`https` URL never consults the registered
`zen.desktop` association at all — it only tries a fixed list of terminal
browsers, all absent, and gives up. **Verdict: FAILED as a live-foreground
demonstration** — this is a gate-host environment gap (no active graphical
session to foreground into), not a defect introduced by this crate; a rerun
once a live Linux desktop session is available on this host is still owed.
It does, however, reproduce a live instance of the same blind spot Leg 2
targets on purpose: `open_external` returned `Ok(())` while nothing opened.

**Leg 2 — no-handler contract (`act_000001a0c493b0f0VxxVfDAE`) — the
predicted gap CONFIRMED.**

*Sub-case A — opener binary genuinely missing.* `PATH` pointed at an empty
directory:

```
$ PATH=<empty-dir> ./call_once https://example.com
open_external("https://example.com") -> Err(NoHandler)
```

Spawn itself fails with `NotFound`, mapped to `NoHandler` exactly as
`desktop.rs` documents. **Verdict: PASSED** — this sub-case matches the
contract precisely.

*Sub-case B — opener present, nothing registered to handle the URL.*
`XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`XDG_DATA_DIRS` redirected to fresh, empty
scratch directories (no `mimeapps.list`, no `applications/` entries), while
the real `xdg-open` stayed on `PATH`. The real `xdg-open` exit status and
the real `open_external` return value, side by side, in the identical
environment:

```
$ XDG_CONFIG_HOME=<empty-scratch>/.config XDG_DATA_HOME=<empty-scratch>/data XDG_DATA_DIRS=<empty-scratch>/data xdg-open https://example.com
Selected DE generic
/usr/bin/xdg-open: line 1045: www-browser: command not found
/usr/bin/xdg-open: line 1045: links2: command not found
/usr/bin/xdg-open: line 1045: elinks: command not found
/usr/bin/xdg-open: line 1045: links: command not found
/usr/bin/xdg-open: line 1045: lynx: command not found
/usr/bin/xdg-open: line 1045: w3m: command not found
xdg-open: no method available for opening 'https://example.com'
$ echo $?
3

$ XDG_CONFIG_HOME=<empty-scratch>/.config XDG_DATA_HOME=<empty-scratch>/data XDG_DATA_DIRS=<empty-scratch>/data ./call_once https://example.com
open_external("https://example.com") -> Ok(())
```

Real `xdg-open` exit status `3` (`EXIT_FAILURE_OPERATION_IMPOSSIBLE` per its
own man page) side by side with `open_external`'s real return value
`Ok(())` — the predicted silent-`Ok(())` gap reproduces exactly as the
module doc in `src/desktop.rs` and `docs/LIMITATIONS.md`'s
`url-launcher-desktop-no-association-unobservable` entry already describe;
this card does not change that documented, deliberate contract, only
confirms it with a real, unfaked `xdg-open`. **Caveat**: on this specific host,
`xdg-open`'s "generic" DE branch (see Leg 1) does not consult the
`mimeapps.list` association at all for `http`/`https` before falling back to
terminal browsers, so this sub-case cannot cleanly isolate "association
missing" from "no display" as two independent causes here — both this test
and the ambient Leg-1 environment hit the same fallback path. A host running
a live KDE/GNOME session (routing through `kde-open5`/`gio` instead) would
be needed to isolate a true "opener present, real desktop session, still no
association" case; that rerun is still owed. **Verdict: PASSED as a
verification** — the return-value/exit-status mismatch this leg exists to
confirm was reproduced with real binaries, not simulated.

**Leg 3 — child reap (`act_000001a0c493b0e4l4qTbnrM`) — PASSED, with a
negative control.** A single long-lived process called the real, fixed
`UrlLauncher::open_external` 24 times in a loop against the real system
`xdg-open`, then inspected its own child table:

```
pid = 850603
call #0 -> Ok(())
[... calls #1-#22 identical ...]
call #23 -> Ok(())
--- ps -o pid,ppid,stat,cmd --ppid 850603 ---
    PID    PPID STAT CMD
 851429  850603 R    ps -o pid,ppid,stat,cmd --ppid 850603
ZOMBIE COUNT: 0
```

Zero `Z`/`<defunct>` entries after 24 real calls. A negative control ran the
literal pre-fix shape (`spawn`, then `drop(child)` with no `wait()` — the
exact code `desktop.rs`'s module doc says used to ship) in the same process
structure, same real `xdg-open`, same 24 iterations:

```
pid = 851907
spawned #0
[... spawned #1-#22 identical ...]
spawned #23
--- ps -o pid,ppid,stat,cmd --ppid 851907 ---
    PID    PPID STAT CMD
 851908  851907 Z    [xdg-open] <defunct>
 851909  851907 Z    [xdg-open] <defunct>
 851910  851907 Z    [xdg-open] <defunct>
 851914  851907 Z    [xdg-open] <defunct>
 851923  851907 Z    [xdg-open] <defunct>
 851924  851907 Z    [xdg-open] <defunct>
 851925  851907 Z    [xdg-open] <defunct>
 851928  851907 Z    [xdg-open] <defunct>
 851929  851907 Z    [xdg-open] <defunct>
 851930  851907 Z    [xdg-open] <defunct>
 851933  851907 Z    [xdg-open] <defunct>
 851934  851907 Z    [xdg-open] <defunct>
 851939  851907 Z    [xdg-open] <defunct>
 851940  851907 Z    [xdg-open] <defunct>
 851944  851907 Z    [xdg-open] <defunct>
 851946  851907 Z    [xdg-open] <defunct>
 851949  851907 Z    [xdg-open] <defunct>
 851950  851907 Z    [xdg-open] <defunct>
 851976  851907 Z    [xdg-open] <defunct>
 851993  851907 Z    [xdg-open] <defunct>
 852007  851907 Z    [xdg-open] <defunct>
 852008  851907 Z    [xdg-open] <defunct>
 852009  851907 Z    [xdg-open] <defunct>
 852014  851907 Z    [xdg-open] <defunct>
    PID    PPID STAT CMD
 852704  851907 R    ps -o pid,ppid,stat,cmd --ppid 851907
ZOMBIE COUNT: 24
```

24 dropped `Child`s, 24 `<defunct>` zombies — with everything else held
identical (same opener, same URL shape, same iteration count, same process),
only the reap-vs-drop difference distinguishes the two runs. **Verdict:
PASSED** — the reap fix is confirmed on the exact platform that found the
leak, with a passing negative control, not just a passing observation.

**`cargo test -p frust-url-launcher`** on this host: 10 passed, 0 failed,
0 ignored (`url::tests::*` × 7, `desktop::tests::*` × 3, the latter only
compiled `#[cfg(all(test, target_os = "linux"))]`).

### Desktop — partially covered

Linux spawn-and-reap and no-handler behaviour is now covered by both this
crate's own unit tests and the on-host device gate above. The macOS `open`
arm and the Windows `ShellExecuteW` arm have never been compiled or run —
see `url-launcher-windows-leg-unrun` in `docs/LIMITATIONS.md` (Windows) and
file a matching entry for macOS if one does not already exist.

[`open_external`]: https://docs.rs/frust-url-launcher/latest/frust_url_launcher/struct.UrlLauncher.html#method.open_external
