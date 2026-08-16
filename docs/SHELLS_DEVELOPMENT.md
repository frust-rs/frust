# Frust - SHELLS Development

Device/emulator gates owned by the SHELLS unit (`frust-shell-common`, `frust-shell-desktop`,
`frust-shell-macos`, `frust-shell-windows`, `frust-shell-linux`, `frust-shell-android`,
`frust-shell-ios`, plus the in-repo platform embedding modules). Shared
prerequisites, build/run commands, the standard verify gate, the mobile compile gates, and the
version-pin policy live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

No gate below has an automated counterpart — each is a person-driven check against an installed
app (`frust run -d <device>` on mobile, a real desktop session otherwise).

## Deep-link manual test (Android)

A device/emulator gate for `nativeOnDeepLink` ([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s
cross-cutting host-signal flow, deep-link) on a project scaffolded with `--deeplink-scheme
<scheme> [--deeplink-host <host>]`, installed via `frust run -d <device>`:

```bash
# Cold start (app not running; queues until the native handle exists):
adb shell am force-stop <package>
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<path>" <package>

# Warm (already foregrounded; singleTop routes via onNewIntent):
adb shell am start -a android.intent.action.VIEW -d "<scheme>://<other-path>" <package>
```

Confirm the app navigates to the linked route both times. iOS (`frust_on_deep_link`) has
a Simulator-only CLI trigger (`xcrun simctl openurl booted "<scheme>://<path>"`); a
physical device has none — tap a registered `CFBundleURLSchemes` link instead.
`frust.toml`'s `[deeplink]` section is informational only, so use `--overwrite` or edit
the platform files directly to change the scheme.

## Safe-area / keyboard / back manual test (Android + iOS)

A device/emulator gate for the inset and back contracts (see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s cross-cutting host-signal flow), against an
installed app (`frust run -d <device>`) — no CLI trigger like the deep-link gate above, so each
is a person-driven check:

- **Safe-area:** rotate the device; confirm top/bottom-anchored content reflows around
  the status bar/notch/gesture-nav insets in both orientations.
- **Keyboard:** focus a bottom text field; confirm content shifts clear of the on-screen
  keyboard, then dismiss it and confirm the layout returns.
- **Back:** press hardware/gesture back on a pushed route; confirm it pops one level,
  and falls through to the platform's own default at the root.
- **Density refresh:** move a running app between displays of different density; confirm
  layout rescales rather than sticking to launch-time density.

## Per-OS desktop shell gate (macOS + Windows + Linux)

A hardware gate for the native integration the shared winit core cannot cover, run against an app
configured with an app name, reverse-DNS id, window icon and menu spec. macOS is closed; Windows
and Linux are still owed (see [LIMITATIONS.md](LIMITATIONS.md) `desktop-shells-runtime-unverified`
for exactly which checks remain). Per host:

- **macOS:** menu bar shows the app-named application menu; ⌘Q and the menu Quit both exit;
  Hide/Show All work; closing with `quit_on_last_window_closed = false` hides the window and a
  Dock click from the *inactive* state brings it back; the zero-config preview is unchanged.
- **Windows:** titlebar and taskbar icons; taskbar grouping under the configured AppUserModelID;
  titlebar brightness follows an app-forced theme flip; the native menu bar and each declared
  accelerator activate their item.
- **Linux:** the window pairs with its `.desktop` entry (Wayland `app_id` / X11 `WM_CLASS`) and
  the X11 window icon shows; run non-headless, on both session types where available.

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `muda 0.19` minor | `frust-shell-macos`'/`frust-shell-windows`'s native menu-bar bindings (`NSMenu`/`HMENU` built from `DesktopConfig::menu_spec`). Pins `objc2 0.6.1` + `objc2-app-kit 0.3.x`, unifying with this workspace's existing `objc2` family; muda's own `gtk`/`libxdo` deps live behind a Linux/BSD-only target table internally, so this pin resolves no GTK dependency on macOS/Windows — `frust-shell-linux` deliberately never depends on it | `cargo check --target aarch64-apple-darwin -p frust-shell-macos && cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`, plus `cargo tree -d` (watch for muda bumping its `objc2` lineage) |
| `windows-sys 0.61` minor | `frust-shell-windows`'s Win32 bindings (`SetCurrentProcessExplicitAppUserModelID`, `TranslateAcceleratorW`/`HACCEL`/`MSG`). Resolved empirically, not guessed: muda 0.19.3's own `>=0.60, <=0.61` constraint picks `0.61.2`, the minor already dominant in this lockfile. No features declared at the workspace level — each consuming crate selects its own list (`frust-shell-windows/Cargo.toml`: `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`) | `cargo check --target x86_64-pc-windows-gnu -p frust-shell-windows`; `cargo tree -d --target x86_64-pc-windows-msvc` — watch for a third `windows-sys` minor appearing beyond the pre-existing `0.52.0`/`0.61.2` pair |

`objc2-app-kit 0.3` (the `NSApplication`/`NSResponder` slice `frust-shell-macos` names directly,
for the Dock-reopen observer) joins the objc2 pin family owned by
[PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md) rather than getting a row here — see its Version
Pins table. `frust-shell-macos`'s own manifest declares it with a literal version and a trimmed
`default-features = false` feature list, not `{ workspace = true }`, because Cargo forbids a
member from overriding `default-features` on a workspace row that doesn't set it; the two must be
kept in step (same workaround as `plugins/clipboard`'s `objc2-ui-kit` row). Known accepted
residual: `objc2-app-kit` resolves to two versions (`0.2.2` via `accesskit_macos`/winit,
`0.3.2` via this pin/`arboard`) — pre-existing, not widened by this pin, do not force-align.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) — the unit's design
- [TESTING.md](TESTING.md) — test tiers, golden images, emulator runbook
