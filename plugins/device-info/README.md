# frust-device-info

A small **device-memory query** for frust apps: total RAM, the Android low-RAM flag,
system-available memory and the iOS per-process headroom. It exists because Android 14 and later
deliver no foreground `onTrimMemory` level any more, so an app on screen has no memory figure to
size caches with.

**Platform support:** Android, iOS, macOS, Linux and Windows. On web and any other target
`DeviceMemory::query()` returns `None`.

Like every frust **platform plugin**, this crate is added to your app's own `Cargo.toml`
alongside `frust`; the `frust` facade does not re-export it.

---

## 1. Add the dependency

Add `device-info` from the `frust` TUI's Add Plugin dialog, or add the dependency yourself:

```toml
# app Cargo.toml — [dependencies]
frust-device-info = { path = "<frust>/plugins/device-info" }  # or the crates.io release, e.g. version = "0.6"
```

```rust,ignore
use frust_device_info::DeviceMemory;

if let Some(memory) = DeviceMemory::query() {
    let gib = memory.total_bytes as f64 / (1u64 << 30) as f64;
    println!("{gib:.1} GiB total");
}
```

No permission, Gradle module or plist key is needed.

---

## 2. `DeviceMemory`

| Field | Android | iOS | macOS | Linux | Windows |
|---|---|---|---|---|---|
| `total_bytes` | `MemoryInfo.totalMem` | `sysctl hw.memsize` | `sysctl hw.memsize` | `MemTotal` | `ullTotalPhys` |
| `low_ram_device` | `isLowRamDevice()` | `false` | `false` | `false` | `false` |
| `system_available_bytes` | `MemoryInfo.availMem` | `None` | `None` | `MemAvailable` | `ullAvailPhys` |
| `process_headroom_bytes` | `None` | `os_proc_available_memory()` | `None` | `None` | `None` |
| `system_low` | `MemoryInfo.lowMemory` | `None` | `None` | `None` | `None` |

---

## 3. Platform caveats

- **Android 14+ delivers no foreground trim level**, so `on_memory_pressure` never fires while
  the app is on screen. Poll this query instead; `system_low` is the one live low-memory state a
  foreground app still gets.
- **`availMem` is not absolute.** It is a system-wide estimate, not this process's budget.
- **iOS headroom is advisory.** It moves constantly; never cache it. On iOS `Some(0)` is a
  reading, not an absence: a frust binary there is always an app, so zero means the app is at or
  over its memory limit.
- **Linux reports the host, not the container.** Inside a container or under a cgroup limit,
  `total_bytes` and `system_available_bytes` come from `/proc/meminfo`, which shows the host's
  memory and not the limit, so they can overstate the room the app has.
- **macOS reports total only** in v1.
- **Web reports nothing.** `navigator.deviceMemory` is Chromium-only, coarse and
  secure-context-gated.

---

## 4. Sizing a working set

Prefer `process_headroom_bytes` where it is present: it is the OS's own figure for what this app
may still allocate. Elsewhere, use a class table of your own choosing keyed on `total_bytes`
(and `low_ram_device`), and take a fraction of `system_available_bytes` as a ceiling when present.

The call is synchronous, cheap and thread-agnostic, but it is a reading, not a signal. Re-query
from `AppLifecycleListener::on_memory_pressure` and when a page opens rather than once at start-up.

On iOS, `process_headroom_bytes == Some(0)` is the headroom-exhausted state: shed memory now. Never
treat it as unknown and fall back to sizing from `total_bytes`; only `None` means the figure is
absent. Callers own the polling cadence. Poll at a modest rate, size conservatively, and apply
hysteresis: shrink when a reading falls below a lower threshold, and grow back only after it stays
above a higher one, so a noisy figure does not thrash the cache. The figures are hints, not limits.

---

## 5. Caveats

- Nothing is cached; every call reads the platform again.
- `DeviceMemory` is `#[non_exhaustive]`; fields may be added.

## Links and license

Documentation: <https://frust.dev>. Source: <https://github.com/frust-rs/frust>.

## License

Licensed under either of the Apache License, Version 2.0 (`LICENSE-APACHE`)
or the MIT license (`LICENSE-MIT`) at your option (SPDX: `MIT OR Apache-2.0`).
Both license files are included beside this README.
