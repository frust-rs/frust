# frust-background-task

Keeps a short piece of work alive after your app leaves the screen. Backs
`frust::BackgroundTask` with a real platform service; without this plugin the
guard is a no-op on Android.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust`; the `frust` facade does not re-export it.

## Add it

```sh
frust add background-task
```

That adds the Cargo dependency and wires the Android Gradle module. Then call
`install()` once, from the app's setup block or the root `Component::init`:

```rust
frust::app!(MyApp, setup = {
    frust_background_task::install();
});
```

Both places run on the UI thread after the reactive runtime exists. `install()`
returns whether a backend was installed, so a repeat call after Activity
recreation returns `false` harmlessly. A `BackgroundTask::begin` before
`install()` is the core no-op guard.

## Use it

App code only ever touches `frust::BackgroundTask`. Take the guard on the UI
thread, inside a flush-on-background callback, and move it to the blocking work:

```rust
let task = BackgroundTask::begin("Saving");
spawn_blocking(move || {
    write_to_disk();
    drop(task);
});
```

| Platform | `install()` | Backend |
|---|---|---|
| Android | `true` | a `shortService` foreground service |
| iOS, desktop, web | `false` | none: the guard stays the core no-op |

## Android

One foreground service covers every live guard. It starts with the first guard
and stops when the last one ends. Task names reach the notification, so keep
them user-readable.

**Notification.** A low-importance notification on channel
`frust_background_work` (app icon, app label, task name). It asks for deferred
display, so on Android 12+ the system may hold it back for up to 10 seconds, and
a short task finishes first. That is not guaranteed. Android 10 and 11 (API
29-30) have no deferral, so a brief flash is possible there.
`POST_NOTIFICATIONS` is not requested: the service runs either way, and the
notification is simply hidden when the permission is denied.

**Refusal.** Android refuses a foreground-service start in some states, for
instance from the background. Then `begin` reports `is_granted() == false` and
`expired()` reads `true`; the work runs anyway, unprotected. Begin the guard
while the app is still visible, which is what a flush-on-background callback
does. A refusal that only arrives once the service runs (`startForeground`
throwing) shows up as `expired()`.

**Time limit.** A 150-second watchdog runs from the first `startForeground`, and
each later guard re-arms it. It sits below the system's 3-minute `shortService`
limit, and is the only limit before Android 14. When it or the system's own
timeout fires, `expired()` turns `true` and the service stops. `expired()` is
your signal to cut the work short; nothing cancels it for you.

## Google Play declaration

The plugin's manifest adds `android.permission.FOREGROUND_SERVICE` and the
`shortService` service to your app. Play requires apps targeting Android 14+ to
declare every foreground service type they use in the Play Console, and lists
no exemption for `shortService`, so file that declaration before you ship.

## Opting out on Android

To keep the plugin (say, for another platform) without the service or the
declaration, remove both entries from your app's manifest:

```xml
<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:tools="http://schemas.android.com/tools">
    <uses-permission
        android:name="android.permission.FOREGROUND_SERVICE"
        tools:node="remove" />
    <application>
        <service
            android:name="dev.frust.backgroundtask.FrustBackgroundTaskService"
            tools:node="remove" />
    </application>
</manifest>
```

Every `begin` is then refused and the work runs unprotected.

## Wiring the Gradle module by hand

`frust add` does this for you. An existing project can add it directly:

```kotlin
// android/settings.gradle.kts
include(":frust-background-task")
project(":frust-background-task").projectDir =
    frustLocalDir("frust.plugin.frust-background-task.dir")

// android/app/build.gradle.kts
implementation(project(":frust-background-task"))
```

If the module is missing, `install()` logs a warning and returns `false`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
