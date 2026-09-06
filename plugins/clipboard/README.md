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

// For content the user wouldn't want another app, a clipboard-history UI,
// or another of their devices to see (tokens, passwords, terminal output) —
// see `## 2. Security` below.
Clipboard::set_text_sensitive("ghp_super_secret_token")?;
```

**No manifest, plist, permission, Gradle module, or Swift package is
needed** — reading/writing plain text requires no OS permission on either
mobile platform, and the Android/iOS backends are plain JNI/`objc2` calls
with no Kotlin/Swift glue to wire in. The frust TUI's **Add Plugin** dialog
still lists this plugin (for a consistent workflow across every plugin), but
all it applies is the Cargo dependency line above.

`Clipboard::get_text()` returns `Ok(None)` for an empty clipboard, one
holding a non-text payload, **or one explicitly set to the empty string
`""`** — never an error. There is no way to distinguish "never set" from
"set to `\"\"`" through this API, and every backend/the conformance suite
hold to this one story.

---

## 2. Security

`Clipboard::set_text` moves `text` into a **world-readable OS channel**:
there is no permission gate on any of the three platforms, so any other app
the user has granted clipboard access to (which, on desktop and pre-13
Android, is implicitly every app) can read it back with its own clipboard
API. Neither `set_text` nor `set_text_sensitive` encrypts the clipboard or
stops a determined reader — both are plain-text writes; the difference is
what each platform does to reduce *incidental* exposure.

Use **`Clipboard::set_text_sensitive`** instead of `set_text` for content
the user would not want visible to another app, a clipboard-history UI, a
screen recording, or another of their signed-in devices — copied terminal
output, tokens, passwords, one-time codes:

- **Android (API 33+):** sets `ClipDescription.EXTRA_IS_SENSITIVE` on the
  clip's description extras, which suppresses the Android 13+ copied-text
  preview toast and signals clipboard-history UIs to redact/omit the entry.
  A no-op fallback to plain `set_text` below API 33 (the extras key is simply
  ignored by the platform, not an error).
- **iOS:** writes via `UIPasteboard.setItems(_:options:)` with
  `.localOnly: true`, which blocks Universal Clipboard/Handoff from
  replicating the pasteboard item to the user's other signed-in Apple
  devices. No expiration date is set by default (a possible future opt-in).
- **Desktop (macOS/Linux/Windows via `arboard`):** `set_text_sensitive` is a
  plain alias for `set_text` on all three — but only X11/Wayland genuinely
  has no sensitivity mechanism to alias away. Windows documents the
  `CanIncludeInClipboardHistory`, `CanUploadToCloudClipboard`, and
  `ExcludeClipboardContentFromMonitorProcessing` clipboard formats
  precisely to keep a clip out of Win+V clipboard history and Cloud
  Clipboard sync — the same cross-device replication channel `.localOnly`
  closes on iOS above. macOS has the community convention
  `org.nspasteboard.ConcealedType` (not OS-documented, unlike Windows'
  formats). Neither is set here: `arboard` exposes no custom-format write
  surface, so reaching either would require `clipboard-win`'s raw format
  API directly — deliberately deferred, not implemented in this crate
  (`docs/LIMITATIONS.md`'s `clip-desktop-sensitivity-noop`).

Use plain `set_text` for anything else (e.g. copying a share link, a
user-composed note) — there is no reason to pay `set_text_sensitive`'s
slightly heavier iOS write path for content that isn't sensitive.

---

## 3. Platform caveats

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

## 4. No blocking calls

Every backend this crate ships is synchronous and non-blocking — unlike
`frust-secure-storage`'s biometric gate or `frust-camera`'s permission/
capture calls, nothing here needs `frust-reactive`'s `spawn_blocking`.
`Clipboard::set_text`/`Clipboard::get_text`/`Clipboard::set_text_sensitive`
are safe to call from the UI thread on every platform (see each backend
module's own rustdoc for the platform documentation this relies on).

---

## 5. Caveats

- **v1 is plain text only.** A byte/image clipboard payload is a future
  enhancement, matching `frust-shared-preferences`'/`frust-secure-storage`'s
  own value-model scoping in v1.
- **`frust create --overwrite` is a non-issue here** — this plugin adds
  nothing to the generated project besides the Cargo dependency line, so
  there is nothing for `--overwrite` to drop.

---

## 6. Testing this crate itself

`desktop::tests::conformance` exercises the **real host clipboard** (the
full conformance suite, plus a `set_text_sensitive`-aliases-`set_text`
check) and is skipped by default (a plain `cargo test --workspace`/`-p
frust-clipboard` must never clobber whatever a developer had copied). Opt in
deliberately when touching this crate's desktop backend:

```bash
FRUST_CLIPBOARD_TESTS=1 cargo test -p frust-clipboard
```

This overwrites your clipboard and leaves a test string in it when it
finishes.
