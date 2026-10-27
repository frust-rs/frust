# frust-connectivity

Default-network availability and change events for frust apps — Android
`ConnectivityManager` and Apple Network.framework (`nw_path_monitor`) behind
one platform-independent API. It answers two questions: *is there a usable
default network right now?* (`Connectivity::current`) and *tell me when that
changes* (`Connectivity::set_listener`).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

## Platform support

| Platform | Backend | Minimum |
|---|---|---|
| Android | `ConnectivityManager` default-network callback | minSdk 29 |
| iOS | Network.framework `nw_path_monitor` | iOS 12 |
| macOS | Network.framework `nw_path_monitor` | macOS 10.14 |
| Linux, Windows, web | none (no-op) | — |

On the no-op platforms the monitor never starts: listeners are accepted and
never called, and `Connectivity::current()` is always `None`. That means
"unknown", not "offline" — don't gate network work on it there.

## Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-connectivity = "0.6"
```

## Use

```rust
use std::sync::mpsc;
use frust_connectivity::{Connectivity, ConnectivityEvent};

let (tx, rx) = mpsc::channel();

// Keep the handle for as long as you want events: dropping it unregisters.
let handle = Connectivity::set_listener(Box::new(move |event| {
    // Runs on the plugin's own thread. Hand off and return; never block.
    let _ = tx.send(event);
}));

// Elsewhere, off the listener thread:
for event in rx.iter() {
    match event {
        ConnectivityEvent::Available { path, initial } => {
            // A usable default network; `initial` marks the first report.
            println!("online over {:?} (metered: {})", path.transport, path.expensive);
        }
        ConnectivityEvent::Lost => println!("offline"),
        _ => {}
    }
}

// Any time, from any thread:
let online = Connectivity::current().is_some();
```

- **`Available { path, initial }`** — a usable default network (Android:
  `INTERNET` + `VALIDATED`; Apple: path satisfied). Reported when the network
  first becomes usable and again when it changes to a different network or
  transport. `initial` is `true` for the first report after the monitor
  starts. A device that starts offline emits nothing until a network appears.
- **`Lost`** — the usable default network went away and nothing replaced it.
- Repeated platform reports of the same state are collapsed: each transition
  arrives once. A change of only `expensive`/`constrained` on the same network
  is not an event; `current()` reflects it.
- `NetworkPath::id` is opaque and process-local — compare it, never persist it.

The monitor starts on the first `set_listener` or `current` call, once per
process. A listener registered after that sees only later transitions; read
`current()` for the state at registration time.

## Threading

The listener runs on one plugin-owned thread, `frust-connectivity-events` —
never the UI thread. It must not block: send the event into a channel or
`frust_reactive::use_task` and return. Events arrive in the order the platform
reported them, to each listener in registration order. The queue behind that
thread is bounded; a listener that blocks long enough loses the newest events
(logged), while `current()` stays correct.

## Permissions

- **Android:** `android.permission.ACCESS_NETWORK_STATE` (a normal,
  install-time permission). It is declared by the plugin's own Gradle module
  manifest and merges into your app — no manifest edit of your own.
- **iOS / macOS:** nothing. Network.framework path monitoring needs no
  entitlement or Info.plist key.

## Limitations

- **No events in the background.** While the app is suspended (iOS) or
  frozen/cached (Android) the OS delivers no network callbacks, so changes
  made then produce no event. Re-check `Connectivity::current()` when the app
  comes back — pair it with `frust::AppLifecycleListener`'s `on_restart` /
  `on_resume`.
- **Default network only.** It reports the one network the OS routes new
  connections over, not every interface, and says nothing about whether a
  particular host is reachable.
- **Usable ≠ fast.** `Available` means validated/satisfied; a captive or
  congested network can still fail requests. Treat it as a hint to retry, not
  a guarantee.
- **The monitor is never stopped** once started.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
