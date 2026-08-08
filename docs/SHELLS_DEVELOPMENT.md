# Frust - SHELLS Development

Device/emulator gates owned by the SHELLS unit (`frust-shell-common`, `frust-shell-desktop`,
`frust-shell-android`, `frust-shell-ios`, plus the in-repo platform embedding modules). Shared
prerequisites, build/run commands, the standard verify gate, the mobile compile gates, and the
version-pin policy live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md).

Neither gate below has an automated counterpart — each is a person-driven check against an
installed app (`frust run -d <device>`).

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

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) — the unit's design
- [TESTING.md](TESTING.md) — test tiers, golden images, emulator runbook
