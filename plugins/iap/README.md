# frust-iap

A platform-independent in-app-purchase API for frust apps — Android Play
Billing (`openiap-google`) and iOS StoreKit (`openiap-apple`/`OpenIAP`),
speaking the [OpenIAP](https://openiap.dev) 3.0.1 wire protocol as JSON
strings across the Rust↔Kotlin/Swift FFI boundary. `Iap` exposes **15 v1
operations**, all plain associated functions (there is exactly one store
connection per process, no handle to hold).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

> **Templates stay clean.** A generated frust project ships **no** IAP code,
> permissions, or store wiring. The `frust` TUI's **Add Plugin** dialog
> applies every §1 step below in one shot (`iap` card — Cargo dependency,
> Android Gradle module, iOS Swift package); the manual steps are the same
> edits, documented for hand-wiring and for auditing what the dialog did.
> On Android the plugin's platform code ships as its own Gradle library
> module rather than files copied into your app, so there is nothing to keep
> in sync by hand once it's wired in; on iOS it ships as its own local Swift
> package, added as a second package reference beside the embedding's
> `FrustEmbedding`.

---

## 1. Add the plugin

### 1a. Cargo (always)

```toml
# app Cargo.toml — [dependencies]
frust-iap = { path = "<frust>/plugins/iap" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

### 1b. Android setup — include this plugin's Gradle module

By hand, exactly the include/projectDir/build-dir-redirect trio every other
frust plugin module uses:

1. `android/settings.gradle.kts`:

   ```kotlin
   include(":frust-iap")
   project(":frust-iap").projectDir =
       file("<frust checkout>/plugins/iap/platform/android")

   gradle.lifecycle.beforeProject {
       if (path == ":frust-iap") {
           layout.buildDirectory.set(rootDir.resolve("build/frust-iap"))
       }
   }
   ```

2. `android/app/build.gradle.kts`:

   ```kotlin
   dependencies {
       implementation(project(":frust-iap"))
   }
   ```

**No manifest edit needed.** The module's own `AndroidManifest.xml` declares
no `<uses-permission>` of its own, but the `openiap-google:3.0.1` dependency
it pulls in (the Play flavor, pinned exact) resolves the Play Billing
Library AAR, which declares `com.android.vending.BILLING` itself — the
manifest merger folds that into your app transitively. The module also
brings `kotlinx-coroutines-android:1.9.0` (pinned to the same version
`openiap-google` itself resolves, so the process never carries two coroutine
runtimes) and ships its own `consumer-rules.pro` (R8 keep rules for
`FrustIapHost`/`FrustIapInitProvider`), so a release build needs nothing
added to your own `proguard-rules.pro`. `FrustIapInitProvider`, a
manifest-declared `ContentProvider` inside the module, bootstraps the
application `Context` at process start — before any Activity exists — so
there is no init call to make from your own Kotlin/Rust code.

`dev.frust.iap.FrustIapHost` is a hard contract (its JNI-exported symbol
names are frozen — see `plugins/iap/src/android.rs`'s module doc) — never
move or rename it.

### 1c. iOS setup — add the plugin's local Swift package

In Xcode: **File → Add Package Dependencies… → Add Local…**, select
`<frust checkout>/plugins/iap/platform/ios`, and add the `FrustIap` product
to the `Runner` target (alongside the existing `FrustEmbedding` package —
SwiftPM dedupes the shared dependency rather than vendoring it twice).

Unlike `plugins/camera/platform/ios` (a purely local package), `FrustIap`'s
own `Package.swift` also resolves an **external** SwiftPM dependency —
`github.com/hyodotdev/openiap`, pinned `exact: 3.0.1` — so the **first**
resolution needs GitHub reachable over the network; Xcode does this
automatically the first time you build (or open the project) after adding
the package.

**No `Info.plist` key is needed** — unlike camera's
`NSCameraUsageDescription`, Apple requires no usage-description string for
StoreKit purchases.

---

## 2. Purchase lifecycle contract

This is the load-bearing section — the plugin's API shape only makes sense
once these rules are clear.

### Open the connection first

`Iap::init_connection` must run before anything else. Every other
store-touching call reports `IapError::NotConnected` before it succeeds, and
again after `Iap::end_connection`. `init_connection` is **idempotent** — a
second call on an already-open connection is `Ok(true)`, never an error.
`end_connection` **never errors**: `Ok(true)` when it closed an open
connection, `Ok(false)` when none was open. Registered purchase listeners
survive an end/init cycle.

### A purchase is request → event → server-verify → finish — bluntly

1. `Iap::request_purchase(props)` returns as soon as the store has
   **accepted** the request. It is non-blocking and callable from any
   thread, including the UI thread. **It never reports whether anything was
   bought.**
2. Register `Iap::set_purchase_listener` **before** the first
   `request_purchase`, and keep the returned `ListenerHandle` alive
   (dropping it unregisters immediately). The listener is the **single
   source of truth**: `IapEvent::PurchaseUpdated(Purchase)` on success,
   `IapEvent::PurchaseError(IapPurchaseError)` on failure — including a
   plain user cancel (`IapErrorCode::UserCancelled`), which is a normal
   flow outcome, not a bug to report. A purchase completed while no
   listener was registered is silently lost to the app until its next
   `Iap::get_available_purchases`.
3. **Verify the purchase server-side**, then grant entitlement. Never grant
   off `request_purchase`'s return.
4. Only then call `Iap::finish_transaction(purchase_input, is_consumable)`.
   `is_consumable` picks acknowledge vs. consume on Android (`None` lets the
   backend decide from the purchase itself — acknowledge for a
   non-consumable/subscription); iOS's only settlement is `finish`, so
   `is_consumable` is advisory there, but pass `Some(true)` for a consumable
   on Android or a second purchase of the same SKU is refused as
   already-owned.

**Android auto-refunds an unacknowledged/unconsumed purchase after ~3
days.** The countdown starts when the purchase completes, not when the app
next launches — settle every purchase, including ones recovered from
`Iap::get_available_purchases` at startup rather than a live event, as soon
as it is durably recorded. iOS has no such deadline, but an unfinished
transaction is **re-delivered to the app forever** until it is finished.

```rust
use frust_iap::{Iap, IapEvent, IapErrorCode};

// Register before the first request_purchase, and keep the handle alive
// for the life of the listener.
let _listener = Iap::set_purchase_listener(Box::new(|event| match event {
    IapEvent::PurchaseUpdated(purchase) => {
        // 1. verify `purchase` server-side, 2. grant entitlement durably,
        // 3. THEN settle it — never on request_purchase's own return.
        let purchase = purchase.clone();
        frust_reactive::spawn_blocking(move || {
            use frust_iap::PurchaseInput;
            Iap::finish_transaction(
                PurchaseInput {
                    id: purchase.id,
                    product_id: purchase.product_id,
                    ids: purchase.ids,
                    transaction_date: purchase.transaction_date,
                    purchase_token: purchase.purchase_token,
                    store: Some(purchase.store),
                    quantity: purchase.quantity,
                    purchase_state: purchase.purchase_state,
                    is_auto_renewing: purchase.is_auto_renewing,
                },
                None, // let the backend decide (Some(true) for a consumable)
            )
        });
    }
    IapEvent::PurchaseError(err) if err.code == IapErrorCode::UserCancelled => {
        // normal flow — the user backed out of the store sheet
    }
    IapEvent::PurchaseError(err) => {
        // a real failure — surface it
        log::warn!("purchase failed: {err}");
    }
}));
```

### iOS: two calls acknowledge dispatch, not outcome

`Iap::request_purchase` (above) and `Iap::deep_link_to_subscriptions` both
return as soon as the Swift bridge has **dispatched** the request — before
the underlying `await` (a StoreKit sheet the user has to deal with)
resolves. A purchase's own outcome still always reaches the app: the store
publishes it on the purchase-error listener even when the awaited call
itself throws. **`deep_link_to_subscriptions` has no such fallback** — a
failure *after* dispatch is logged by the Swift glue's own console output
and reported nowhere else on the Rust side. This is a documented, accepted
blind spot (`plugins/iap/src/apple.rs`'s module doc), not something to route
around.

### iOS replay: dedup is per-session, not per-purchase

The iOS bridge registers the purchase-updated listener with
`dedupeTransactionIOS: true`, so one transaction id is delivered at most
**once per `init_connection`/`end_connection` session**. StoreKit still
replays an **unfinished** transaction on the next app launch (a new
session) — an app may legitimately see the same purchase again after a
kill/relaunch. `Iap::finish_transaction` is what stops that; there is
deliberately **no cross-launch dedupe** in this crate (it would need
persistence outside the plugin's charter). This is standard StoreKit
behavior — handle it with idempotent server-side verification, the same way
any StoreKit app does.

### Pairing with `spawn_blocking`

Every operation except `request_purchase` and `set_purchase_listener`
blocks its caller (a Play Billing round trip or a StoreKit `async` query).
Pair each with `frust_reactive::spawn_blocking`, and never call one on the
platform UI thread — every backend fails fast with `IapError::UiThread`
instead of parking there:

```rust
let connected = frust_reactive::spawn_blocking(|| Iap::init_connection(None)).await??;
```

The guard order every call runs, before any platform work: **pre-init
readiness** (`IapError::PlatformNotInitialized`) → **UI-thread fail-fast**
(`IapError::UiThread`) → **connection state** (`IapError::NotConnected`).
`request_purchase` is the one exception with no UI-thread guard — Play
Billing's `launchBillingFlow` and StoreKit's purchase sheet are themselves
main-thread/main-actor APIs, so each backend reaches that thread on its own
rather than refusing the caller.

Android splits its bound in two: `request_purchase`'s dispatch-ack — the one
UI-thread-callable call — gets a short **5s** timeout (`android.rs`'s
`ACK_TIMEOUT`), since nothing on that path talks to the store, only decodes
arguments, resolves the current Activity, and detaches the flow. Every other
Android round trip, including `deep_link_to_subscriptions`'s **full** await
(Android does **not** ack it the way iOS does — the dispatch waits for
`OpenIapStore.deepLinkToSubscriptions` to return before answering, and the
call is UI-thread-refused like the rest of this group), uses a uniform **60s**
timeout (`android.rs`'s `HOST_CALL_TIMEOUT`) before reporting
`IapError::Platform` — a documented "the answer is never coming" bound, not
a performance budget. iOS varies the bound by call: **30s** for a normal
store query, **120s** for `restore_purchases` (which may present an App
Store password prompt), and a short **5s** ack-only wait for
`request_purchase`/`deep_link_to_subscriptions` (`apple.rs`'s `ACK_TIMEOUT`),
since neither talks to the store on that path — only decodes arguments and
starts a task.

### The 15 operations

| Operation | Blocks? | Platforms | Notes |
|---|---|---|---|
| `init_connection` | yes | Android, iOS | idempotent |
| `end_connection` | yes | Android, iOS | never errors; reports whether it closed anything |
| `fetch_products` | yes | Android, iOS | a SKU the store doesn't know is omitted, not an error |
| `get_available_purchases` | yes | Android, iOS | the restore/entitlement query — not how a fresh purchase arrives |
| `get_active_subscriptions` | yes | Android, iOS | |
| `has_active_subscriptions` | yes | Android, iOS | boolean shortcut, always consistent with the above |
| `get_storefront` | yes | Android, iOS | not normalized between the two platforms |
| `request_purchase` | **ack only** | Android, iOS | outcome via the listener — see above; the one UI-thread-callable call, 5s ack timeout both platforms |
| `finish_transaction` | yes | Android, iOS | the 3-day Play deadline governs this call |
| `restore_purchases` | yes | Android, iOS | recovered purchases surface via the listener + `get_available_purchases`, not this call's return |
| `deep_link_to_subscriptions` | Android: yes / iOS: **ack only** | Android, iOS | both platforms UI-thread-refused; Android: full await, 60s bound; iOS: ack only, 5s bound, post-dispatch failure is log-only |
| `acknowledge_purchase` | yes | **Android-only** | `IapError::NotSupportedOnPlatform` elsewhere |
| `consume_purchase` | yes | **Android-only** | `IapError::NotSupportedOnPlatform` elsewhere |
| `get_pending_transactions` | yes | **iOS-only** | `IapError::NotSupportedOnPlatform` elsewhere |
| `set_purchase_listener` | no | Android, iOS (desktop: registers, never fires) | dropping the handle unregisters |

---

## 3. Store setup (account-dependent)

Everything in this section needs a real developer account. If you don't
have one set up yet, skip to §4 — the local StoreKit-Testing route needs no
account at all.

### Android — Play Console

1. Create the app in Play Console, matching your `applicationId`.
2. Add in-app products (one-time) and/or subscriptions under Monetize →
   Products, using the SKUs your app requests.
3. Add license testers under Setup → License testing — their Google account
   can complete a real purchase flow without being charged.
4. Upload a build to a testing track (internal, closed, or open).
   **Purchases only work on a build installed from a Play testing track** —
   not a debug APK you `adb install`ed directly. A tester opts in via the
   track's tester link and installs through the Play Store app itself.

### iOS — App Store Connect

1. Create the in-app purchase products in App Store Connect, matching your
   bundle id and the SKUs your app requests.
2. **In-App Purchase capability rides normal App Store provisioning** —
   unlike push notifications or HealthKit, there is no separate Xcode
   capability toggle or entitlement to add; any app with a standard
   provisioning profile can call StoreKit.
3. Testing against real App Store Connect products needs a **sandbox
   tester** Apple ID (App Store Connect → Users and Access → Sandbox
   Testers), signed into the Sandbox on the test device — never your real
   Apple ID.

---

## 4. Testing without a store account

### iOS — the `.storekit` local configuration

1. In Xcode: **File → New → File… → StoreKit Configuration File** (e.g.
   `Products.storekit`), added to your app target.
2. Add products in its editor — consumable, non-consumable, or subscription
   — using the SKUs your app requests. They don't need to exist in App
   Store Connect yet.
3. Enable it for local runs: **Product → Scheme → Edit Scheme… → Run →
   Options → StoreKit Configuration**, and select the file (it defaults to
   **None**).
4. Run on the Simulator or a device. `Iap::fetch_products`,
   `Iap::request_purchase` + the listener, and `Iap::finish_transaction` all
   resolve against this local StoreKit test environment — no App Store
   Connect listing, no sandbox tester account, no network round trip to
   Apple.

**Simulator caveat: logic only, no picture.** The iOS Simulator's GPU can't
satisfy vello 0.9's rendering requirement, so `frust run` on the Simulator
builds/installs/launches but the window stays black
(`docs/DEVELOPMENT.md`'s Known Issues, *iOS Simulator cannot render*) — a
render-stack limitation, not this plugin's. The StoreKit-Testing calls above
still run and can be exercised (log output, a debug harness, a breakpoint),
but a pixel-accurate purchase-flow check needs a physical device.

### Android — no offline equivalent

Play Billing has nothing like `.storekit` — every real call needs a
signed-in Play Store on the device and, ultimately, a testing-track build
(§3). Without a store account, what you *can* verify:

- **Compile gates.** The Android compile gate
  (`cargo check --target aarch64-linux-android -p frust-iap`) and a real
  Gradle build (`:frust-iap:compileReleaseKotlin`, or `frust build apk
  --debug`) prove the module wires in cleanly.
- **`init`/`fetch` error paths.** `Iap::init_connection`/`Iap::fetch_products`
  against a device/emulator with no Play Store, or before Play Console
  setup, report a typed `IapError::Store`/`IapError::Platform` instead of
  hanging or panicking — worth a smoke check even without real products.

---

## 5. Platform notes

### Desktop: a v1 deferral, not a capability gap

Every operation on macOS/Linux/Windows reports
`IapError::NotAvailable(Unavailability::UnsupportedPlatform)` —
deliberately, and unlike `frust-haptics`'s permanent desktop gap: macOS
ships the Mac App Store's own StoreKit and Windows ships the Microsoft
Store's API, so a real desktop backend is a buildable future addition that
only mobile-first v1 scope keeps out today (Linux alone has nothing to
route to). `set_purchase_listener` still works everywhere — it is
in-process bookkeeping reaching no store — it simply never fires on
desktop.

### Platform-specific operations

| Operation | Android | iOS |
|---|---|---|
| `acknowledge_purchase` / `consume_purchase` | works | `IapError::NotSupportedOnPlatform` |
| `get_pending_transactions` | `IapError::NotSupportedOnPlatform` | works |
| every other operation | works | works |

Android's `acknowledge_purchase`/`consume_purchase` turn a Play `false`
response into a typed `IapError::Platform` rather than reporting success —
a silent no-op there would let Play's 3-day auto-refund revoke an
entitlement the app already granted, so this is a deliberate
revenue-protecting check, not an oversight.

### Event-thread contract

An `IapEvent` arrives on **the plugin's own event-delivery thread** — one
thread per process, spawned on the first event and shared by both
platforms — **never** the UI thread, and never the thread that called
`request_purchase`. The hop belongs to this plugin: each host forwards on
whatever thread the store reported on (Play Billing dispatches its purchase
listener on the main thread; OpenIAP's iOS listeners are invoked on the main
actor), and the Rust side queues the event instead of running your callback
there.

A registered listener must not block, must not write a signal directly, and
must not call back into any of `Iap`'s blocking API from inside the
callback — hand the event off (`frust_reactive::use_task`, a channel, or a
signal write scheduled back onto the UI thread) and return immediately.
Multiple listeners are supported; each sees every event, in registration
order, and events arrive in the order the store reported them (one consumer,
first in first out). A slow listener delays the listeners behind it *and*
every later event.

A purchase failure the store publishes is delivered **exactly once**: the
OpenIAP host publishes it on its own error listener, and neither platform's
glue re-reports it. The single exception is a backstop rather than a second
delivery — the iOS glue synthesizes an error event for a post-dispatch throw
that is not a `PurchaseError`, a shape upstream never produces today and one
that would otherwise reach the app as nothing at all.

---

## 6. Manual verification gate

A device/emulator gate for `frust-iap`, against an app that depends on the
plugin per §1 above (via the `frust` TUI Add Plugin dialog, or the same
edits by hand):

- **Add Plugin scaffold builds clean, both platforms.** Apply §1 to
  a fresh scaffold; confirm the Android Gradle build picks up the
  transitive `com.android.vending.BILLING` permission, and the iOS package
  resolves (the first build needs network reachability for the external
  OpenIAP SPM dependency).
- **Minified release build survives R8 (Android).** `(cd
  examples/glyph-catalog/android && ./gradlew :app:minifyReleaseWithR8)`
  proves `consumer-rules.pro` is merged and effective under real
  minification — `FrustIapHost`/`FrustIapHost$*`/`FrustIapInitProvider` and
  the native methods must appear in
  `app/build/outputs/mapping/release/seeds.txt`, never in that variant's
  `usage.txt`.
- **StoreKit-Testing smoke (iOS).** With a `.storekit` configuration
  enabled (§4): `init_connection` succeeds, `fetch_products` returns the
  configured catalog, and `request_purchase` + the registered listener +
  `finish_transaction` complete a full purchase round trip with no store
  account.
- **(when a real store listing exists) sandbox purchase + finish, on a
  physical device.** A Play internal-testing-track install or an App Store
  Connect sandbox tester (§3) completes a real purchase; `finish_transaction`
  settles it within the window; the entitlement persists.
- **Kill/relaunch replay check.** Force-quit the app mid-flow, or leave a
  transaction unfinished, then relaunch: confirm the app is handed the
  purchase again (the listener on iOS, `get_available_purchases` on Android
  at minimum) and can finish it — the behavior §2's *iOS replay* and
  *3-day deadline* sections both depend on being handled, not ignored.
