# frust-clipboard

A platform-independent, **synchronous, plain-text** clipboard for frust apps
— Android `ClipboardManager` over plain JNI, iOS `UIPasteboard` via
`objc2-ui-kit`, and macOS/Linux/Windows via `arboard`. Unlike
`frust-secure-storage`/`frust-shared-preferences`, there is exactly one OS
clipboard, so this plugin has no named-store handle to open: `Clipboard::set_text`/`Clipboard::get_text`
are plain associated functions.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

---

## 1. Add the dependency (the only step)

```toml
# app Cargo.toml — [dependencies]
frust-clipboard = { path = "<frust>/plugins/clipboard" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

```rust
use frust_clipboard::Clipboard;

Clipboard::set_text("copied from frust")?;
let text = Clipboard::get_text()?;   // Some("copied from frust")
```

**No manifest, plist, permission, Gradle module, or Swift package is
needed** — reading/writing plain text requires no OS permission on either
mobile platform, and the Android/iOS backends are plain JNI/`objc2` calls
with no Kotlin/Swift glue to wire in. The frust TUI's **Add Plugin** dialog
still lists this plugin (for a consistent workflow across every plugin), but
all it applies is the Cargo dependency line above.

`Clipboard::get_text()` returns `Ok(None)` for an empty clipboard or one
holding a non-text payload — never an error.

---

## 2. Platform caveats

- **Android 10+ focus gate (read-only).** `getPrimaryClip` returns `null`
  — read back here as `Ok(None)`, the same as a genuinely empty clipboard —
  when the calling app is not the one currently in focus. This is a
  documented Android privacy restriction against background apps snooping
  the clipboard, not a bug; there is no workaround (nor should there be).
  Writing (`set_text`) carries no such restriction.
- **iOS 14+ paste banner.** The first time your app reads the pasteboard
  after gaining focus, iOS shows a one-time system banner ("*App* pasted
  from *Other App*") — a privacy notice, not an error, and not something
  this crate can suppress. `UIPasteboard`'s `detectPatterns`/`detectValues`
  APIs exist to probe pasteboard content without triggering the banner, but
  are out of scope for this crate's plain `get_text`/`set_text` API.
- **X11/Wayland clipboard lifetime (Linux desktop).** On X11 and Wayland the
  clipboard's content is served live by whichever process last claimed
  ownership of the selection; when that process exits, the content
  disappears unless a clipboard manager (`klipper`, `xfce4-clipman`,
  `CopyQ`, …) is running to adopt it first. This is documented `arboard`/X11
  behavior, not a Frust bug — a short-lived CLI/desktop-preview process that
  calls `set_text` and exits immediately may lose that content on a system
  with no clipboard manager running. This crate does not install a lifetime
  workaround (no background daemon, no blocking wait for an overwrite) to
  paper over it.
- **Wayland without `wayland-data-control`.** This crate does not enable
  `arboard`'s `wayland-data-control` feature (native Wayland clipboard
  support via `wl-clipboard-rs`) to keep the dependency graph minimal; a
  pure-Wayland session with no XWayland compatibility layer may therefore be
  unable to reach the clipboard at all. A future opt-in, not enabled today.

---

## 3. No blocking calls

Every backend this crate ships is synchronous and non-blocking — unlike
`frust-secure-storage`'s biometric gate or `frust-camera`'s permission/
capture calls, nothing here needs `frust-reactive`'s `spawn_blocking`.
`Clipboard::set_text`/`Clipboard::get_text` are safe to call from the UI
thread on every platform (see each backend module's own rustdoc for the
platform documentation this relies on).

---

## 4. Caveats

- **v1 is plain text only.** A byte/image clipboard payload is a future
  enhancement, matching `frust-shared-preferences`'/`frust-secure-storage`'s
  own value-model scoping in v1.
- **`frust create --overwrite` is a non-issue here** — this plugin adds
  nothing to the generated project besides the Cargo dependency line, so
  there is nothing for `--overwrite` to drop.
