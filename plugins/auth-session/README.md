# frust-auth-session

An **RFC 8252 "OAuth for native apps"** in-app browser authentication-session
plugin for frust apps — Android Custom Tabs / Apple
`ASWebAuthenticationSession` — presented over the request's `https`
authorization URL and resolving to the identity provider's
`callback_scheme://…` redirect.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

**Status:** this crate ships real Android (Chrome Custom Tabs) and Apple
(`ASWebAuthenticationSession`) backends alongside its host-testable core —
the public API, the `oneshot` awaitable future, the one-live-session
("Busy") guard, and the URL/callback-scheme validators. See § 3 below for
each platform's caveats.

---

## 1. Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-auth-session = { path = "<frust>/plugins/auth-session" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote. The Android backend needs
this plugin's own Gradle library module linked too (Chrome Custom Tabs,
reached through `androidx.browser.customtabs`) — run
`frust plugin add auth-session` to wire the Cargo dependency above and the
Gradle module include together, or add the module by hand per
[`platform/android/build.gradle.kts`](platform/android/build.gradle.kts)'s
own header comment (a `settings.gradle.kts` module include plus an
`implementation(project(":frust-auth-session"))` line in `android/app/`,
the same shape `plugins/secure-storage/platform/android` uses).

On **iOS** the app target must also link `AuthenticationServices.framework`
— `frust plugin add auth-session` does this (its `IosFramework` contribution
appends `-framework AuthenticationServices` to every `OTHER_LDFLAGS` list in
`ios/Runner.xcodeproj/project.pbxproj`); an app wired by hand adds the same
flag itself. This crate's Rust code references the framework's
`ASWebAuthenticationSessionErrorDomain` symbol, and a Rust staticlib cannot
carry a framework link into Xcode's link step, so without the flag the app
fails to link with `Undefined symbols for architecture arm64:
"_ASWebAuthenticationSessionErrorDomain"`. A macOS binary linked by cargo
needs nothing extra — the objc2 binding's own link attribute applies there.

---

## 2. API and awaiting the future

```rust
use frust_auth_session::{AuthSession, AuthSessionOutcome, AuthSessionRequest};

let req = AuthSessionRequest {
    url: "https://issuer.example/authorize?client_id=…&redirect_uri=myapp://cb".to_string(),
    callback_scheme: "myapp".to_string(),
    ephemeral: false,
};

// Poll from the UI thread's local executor (`frust::spawn_local`) or any
// other executor — resolution is driven by the platform's own main thread,
// not by whichever thread happens to be polling.
match AuthSession::start(req).await {
    // `{:?}` on an outcome prints `Callback { url_len: N }`, never the URL —
    // read the String through the value, as here.
    Ok(AuthSessionOutcome::Callback(url)) => {
        // Parse the callback URL and verify the state parameter against your
        // session state (PKCE or equivalent is also required — see § 3.4).
        // Then extract the authorization code from `url`'s query string and
        // exchange it for tokens — this crate's job ends at handing back the
        // raw callback URL.
        let url_str = format!("{}", url);  // read the String through match/pattern
        // ... verify state_param from url_str matches your session ...
        // ... verify PKCE challenge ...
    }
    Ok(AuthSessionOutcome::Cancelled) => { /* user dismissed the tab */ }
    Err(err) => { /* AuthSessionError::{InvalidUrl,Busy,PlatformNotInitialized,NoHandler,Platform} */ }
}
```

`AuthSession::start` validates `req.url` (must be an absolute `https` URL)
and `req.callback_scheme` (1-64 bytes, lowercase-alpha first byte,
lowercase-alnum/`+`/`-`/`.` thereafter, not one of the well-known schemes
`http`/`https`/`file`/`javascript`/`data`/`blob`/`intent`/`content`/`about`)
before touching any platform API — a rejected request resolves
`Err(AuthSessionError::InvalidUrl)` on the future's very first poll.

---

## 3. Platform caveats

- **iOS / macOS** (`ASWebAuthenticationSession`):
  - iOS apps link `AuthenticationServices.framework` (§ 1 — `frust plugin
    add` wires it; a hand-wired app adds the flag itself).
  - Your app's `Info.plist` must register `req.callback_scheme` under
    `CFBundleURLTypes` — the same scheme registration any custom-scheme deep
    link needs.
  - The session intercepts the redirect **in-process**: it never reaches the
    app as a URL open, so `frust_on_deep_link` is **not** invoked for it
    (unlike Android — see § 5). An app's ordinary deep-link handling can
    leave that scheme untouched.
  - This crate's deployment floor is iOS 15 (the workspace floor —
    [DEVELOPMENT.md](../../docs/DEVELOPMENT.md)'s Version-Pin Policy table);
    the deprecated `-initWithURL:callbackURLScheme:completionHandler:`
    initializer is used rather than iOS 17.4's HTTPS-App-Link-capable one
    (`auth-session-ios-https-callback-not-supported-v1` in
    [LIMITATIONS.md](../../docs/LIMITATIONS.md)).
  - Starting a session before any window exists resolves
    `AuthSessionError::Platform("no window to present the authentication
    session over")` rather than presenting nothing.
  - `AuthSessionRequest::ephemeral` maps straight onto
    `prefersEphemeralWebBrowserSession` and is honoured strictly (not
    best-effort, unlike Android below).
- **Android** (Chrome Custom Tabs):
  - Needs this plugin's own Gradle module linked (§ 1) — a session on an app
    that skips it resolves `AuthSessionError::Platform` naming `frust plugin
    add auth-session`.
  - The Custom Tab launches from the currently resumed `Activity`; with none
    resumed the session resolves `AuthSessionError::Platform("no resumed
    Activity …")` rather than queuing.
  - The redirect is observed through the app's own `VIEW`/`BROWSABLE`
    custom-scheme `<intent-filter>` on resume — the same intent-filter shape
    (and same lack of domain verification) an ordinary deep link uses
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](../../docs/LIMITATIONS.md)). The callback URL therefore
    reaches the app **twice** — see § 5.
  - **Provider selection** (`pinCustomTabsProvider`): the user's default
    `http://` `VIEW` handler is pinned if it alone answers `CustomTabsService`;
    otherwise the host enumerates every installed `VIEW`/`BROWSABLE` `http://`
    handler with `MATCH_ALL` (default-app filtering would hide every
    non-default browser), intersects that set with a fixed allow-list of
    well-known browsers in preference order (Chrome variants, Firefox / Fenix /
    Fennec F-Droid, Brave, Samsung Internet, Edge, Vivaldi, DuckDuckGo) and
    pins the first one `getPackageName(…, ignoreDefault = true)` accepts —
    never a package outside the allow-list. With no qualifying provider the
    Custom Tab launches unpinned and the OS resolves it like any `VIEW` intent
    (a plain browser window). The module manifest's `<queries>` block is what
    makes those handlers visible under Android 11+ package-visibility rules.
  - **`NoHandler`:** arises only when `CustomTabsIntent.launchUrl` raises
    `ActivityNotFoundException` — no `VIEW` handler for the authorization URL
    exists at all on the device (very rare; see kind 2 in § 5).
  - `ephemeral` maps to `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`,
    advisory to the browser (a browser that doesn't support it opens a normal
    tab instead of refusing).
  - **Any Activity resume ends the pending session.** The host reads the
    outcome from the resumed Activity's `Intent`: a callback-scheme `Intent`
    is the redirect (`Ok(Callback)`); anything else — a permission dialog, an
    app switch, a system event — resolves `Ok(Cancelled)`, exactly as a user
    dismissing the tab does, and a still-open Custom Tab is left for the user
    to close (`auth-session-android-resume-means-cancelled-v1` in
    [LIMITATIONS.md](../../docs/LIMITATIONS.md)). Error kind 4 (`Platform`,
    "no resumed Activity") is launch-time only: no tab was ever opened.
  - **Attribution is by timing, not evidence.** The host stamps whichever
    callback-scheme `Intent` it observes with the generation of the session
    pending *now*; the generation never travels through the browser or the
    redirect. It only lets Rust discard a result for a session that is no
    longer live (dropped future, already resolved) — it cannot tell a
    superseded tab's late redirect from the live session's own
    (`auth-session-android-callback-rides-intent-filter` in
    [LIMITATIONS.md](../../docs/LIMITATIONS.md)). PKCE + `state` (§ 3.4) are
    the caller's defence. Dropping the awaited future releases the Busy slot
    but does not close the tab (`auth-session-no-cancel-v1`).
- **Desktop (Linux, Windows):** permanent — no platform authentication user
  agent exists; every session rejects with `AuthSessionError::NoHandler`
  (`auth-session-linux-windows-unavailable-v1` in
  [LIMITATIONS.md](../../docs/LIMITATIONS.md)). Drive the same OAuth flow
  through [`frust-url-launcher`](../url-launcher/README.md)'s ordinary
  system-browser launch instead, with a local loopback HTTP listener (or an
  equivalent out-of-band step) receiving the redirect — RFC 8252's own
  "loopback IP redirection" pattern for platforms with no in-app
  browser-tab primitive.

---

## 3.4. Security

This crate returns the raw callback URL to the caller — it does not validate or consume the
authorization code or state parameter on any platform. **Callers MUST use PKCE (RFC 7636 with
S256 challenge method) and MUST verify the `state` parameter against their own session state
before exchanging the code for tokens on EVERY platform.** 

RFC 8252 § 8.1 documents the risks of custom-scheme redirects: Android accepts any Intent carrying
the callback scheme while a session is pending, so any installed app or tapped link can forge a
redirect; iOS/macOS intercept the redirect in-process (in-app only, not as a deep link), but the
redirect itself is unauthenticated — the crate has no way to prove it came from the identity
provider rather than the browser or another app. PKCE and state verification are essential on all
three platforms to prevent authorization code theft.

---

## 4. One-session contract

At most one `AuthSession` runs at a time, process-wide: starting a second one
while the first is still live resolves immediately to
`AuthSessionError::Busy` rather than queuing or replacing it (an in-app
browser tab is a modal, single-instance UI surface on every backend
platform). The slot frees the moment the live session resolves — by outcome,
by error, or by a backend module failing without ever calling back — so a
stuck session can never wedge every later one behind it. Dropping the awaited
future frees the slot too, but frees only this crate's bookkeeping: the
platform UI stays up (Android: until the user closes the tab; iOS/macOS: until
the user dismisses the sheet or the next `start` cancels it) —
`auth-session-no-cancel-v1` in [LIMITATIONS.md](../../docs/LIMITATIONS.md).

---

## 5. Deep links vs the future

On Android, the callback URL Custom Tabs redirects back into the app reaches
the platform **twice**: once as the redirect this crate resolves
`AuthSession::start`'s future with, and once more as an ordinary deep link
`frust::deep_links()` also observes (the OS has no way to know only this
crate wants that intent). Treat the future as the single authoritative
source for an auth-session callback, and ignore `callback_scheme` in your
app's own deep-link handling — acting on both delivers the same
authorization code twice. iOS/macOS has no such double delivery — see § 3.

---

## 6. Device gate

### Android — PASSED with one recorded deviation, 2026-09-22

Device: Xiaomi 12 (`2201123G`, codename `cupid`), LineageOS 23.2
(`23.2-20260604-NIGHTLY-cupid`, build `BP4A.251205.006`), Android 16
(`ro.build.version.sdk` = 36), connected over network `adb`. Browsers
installed: `org.lineageos.jelly` (the device's default `https` handler — no
Custom Tabs support), `org.mozilla.fennec_fdroid` 129.0.0 and
`com.android.chrome` 153.0.8010.49 (both answer `CustomTabsService`).
App: the `examples/playground` debug APK (`it.f0x.playground`) built with
`frust build apk --debug` and installed with `adb install -r`. Gate pages
served from this checkout's `plugins/auth-session/gate/` over a self-signed
https origin on the LAN (`https://192.168.1.109:8443/`, typed into the page's
Base URL field); the first Custom Tab therefore shows Fennec's certificate
interstitial, dismissed once with *Advanced… → Accept the Risk and Continue*.
`adb logcat -v time ActivityTaskManager:I AndroidRuntime:E *:S` captured
alongside.

**Merged manifest** — `adb shell dumpsys package it.f0x.playground` shows both
`<queries>` intents this module contributes (`android.support.customtabs.action.CustomTabsService`
and `android.intent.action.VIEW` + `BROWSABLE` + `https`) merged into the app:

```
queriesIntents=[Intent { act=android.support.customtabs.action.CustomTabsService },
                Intent { act=android.intent.action.VIEW cat=[android.intent.category.BROWSABLE] dat=https: }, …]
```

**Provider selection (three gate-driven fixes, all before the passing run
below).** The first attempt launched the default browser, not a Custom Tab:

```
START u0 {act=android.intent.action.VIEW dat=https://192.168.1.109:8443/... cmp=org.lineageos.jelly/.MainActivity} … from uid 10587 (it.f0x.playground)
```

`CustomTabsIntent.launchUrl` alone resolves to the default `VIEW` handler,
and `CustomTabsClient.getPackageName(context, null)` only probes that default
handler — on this device neither reaches a Custom Tabs provider. The host now
enumerates the installed `https` handlers (which needs the second `<queries>`
intent above under Android 11+ package visibility) and offers them to
`getPackageName`, pinning the Intent to the first provider that answers
`CustomTabsService`; with none installed the plain `VIEW` fallback stands.
After the fix:

```
START u0 {act=android.intent.action.VIEW dat=https://192.168.1.109:8443/... pkg=org.mozilla.fennec_fdroid cmp=org.mozilla.fennec_fdroid/org.mozilla.fenix.customtabs.ExternalAppBrowserActivity}
```

Fennec was chosen over Chrome because it sorts first among the candidates; a
device whose *default* browser supports Custom Tabs keeps its default.
Post-run, the provider-selection policy was tightened to use an allow-list of
well-known providers; Fennec F-Droid remains on that list.

**(1) Callback** — tap `Callback` (Ephemeral off). The Custom Tab loaded
`callback.html?scheme=frustplay` (server log: `192.168.1.114 "GET
/callback.html?scheme=frustplay HTTP/1.1" 200`), redirected, and the app
came back through its own intent filter:

```
START u0 {act=android.intent.action.VIEW cat=[android.intent.category.BROWSABLE] dat=frustplay://auth/... flg=0x10000000 cmp=it.f0x.playground/.MainActivity}
Displayed it.f0x.playground/.MainActivity for user 0: +99ms
```

Same process throughout (`pidof` unchanged). Labels, verbatim:

```
status: Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
latest deep link: frustplay://auth/callback?code=x&state=gate
is_supported: true
```

The second line is the documented double delivery — the same Intent also
reached `frust::deep_links()`. (Playground-only observation: the callback
Intent re-rendered the app on its first section; the `Auth` section's labels
were intact when re-opened.)

**(2) Cancel** — tap `Cancel test`; the Custom Tab opened `cookie.html`;
closed with the tab's **X** → `status: Cancel test -> Ok(Cancelled)`.
Repeated and closed with the system **Back** key → `status: Cancel test ->
Ok(Cancelled)`.

**(3) Busy** — tap `Busy` → `status: Busy -> Err(Busy)` (the second call's
result, written synchronously); the first session's tab opened and, its page
being cached, redirected straight back, so the first future resolved to its
callback while the label kept the `Busy` outcome.

**(4) Ephemeral** — Ephemeral OFF, `Cookie check` twice: the page printed
`frustauth=1 (persisted from an earlier session)` on the second visit
(cookie persisted, as expected). Ephemeral ON, `Cookie check` twice: **both**
visits printed `frustauth=1 (persisted from an earlier session)` — the
provider selected on this device (Fennec F-Droid 129.0.0) does **not** honour
`CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`, which is advisory to
the browser. Each session closed with **X** → `status: Cookie check ->
Ok(Cancelled)`. Chrome 153 was installed but never selected here, so
ephemeral browsing under Chrome remains unobserved. Recorded in
[LIMITATIONS.md](../../docs/LIMITATIONS.md) as
`auth-session-android-ephemeral-browser-dependent`.

**(5) Negative / no-provider device** — a Pixel 4a (LineageOS 23.2, only
`org.lineageos.jelly` installed) was attached but locked, so the plain-`VIEW`
fallback path was not exercised on it. `is_supported()` = `true` on the
Xiaomi (label above).

### Android — round-1 re-check, 2026-09-22 (same device)

After the review round-1 fixes (generation-carrying JNI contract, `pending`
cleared on failure, allow-listed provider selection) the Callback and cookie
steps were re-run with the rebuilt APK.

- **Provider selection regression, fixed before the run.** The first
  round-1 build launched the default browser again: enumerating the `https`
  handlers with `MATCH_DEFAULT_ONLY` returns only the user's default browser
  once one is set, so the allow-list intersection was empty. The query now
  uses `MATCH_ALL` (the allow-list, not the flag, is the trust boundary), after
  which Chrome — first on the allow-list — was pinned:

  ```
  START u0 {act=android.intent.action.VIEW dat=https://192.168.1.109:8443/... pkg=com.android.chrome cmp=com.android.chrome/org.chromium.chrome.browser.customtabs.CustomTabActivity}
  ```

- **Callback (Chrome 153).** After Chrome's own certificate interstitial
  (self-signed LAN origin) the page loaded; Chrome does not follow a
  script-initiated custom-scheme navigation without a gesture and instead
  showed *Continue to Playground?* — tapping the page's visible fallback link
  delivered `frustplay://auth/...` to the app:

  ```
  START u0 {act=android.intent.action.VIEW cat=[android.intent.category.BROWSABLE] dat=frustplay://auth/... cmp=it.f0x.playground/.MainActivity}
  status: Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
  latest deep link: frustplay://auth/callback?code=x&state=gate
  ```

  (The playground now prints the URL through the outcome value; the plugin's
  `Debug` output is `Callback { url_len: 43 }`.)

- **Ephemeral (Chrome 153) — honoured.** Ephemeral OFF, `Cookie check` →
  `no cookie (set now)` (Chrome's normal jar had no cookie yet). Ephemeral
  ON, `Cookie check` → `no cookie (set now)` — the ephemeral session did not
  see the cookie the normal jar had just stored. Ephemeral OFF again →
  `frustauth=1 (persisted from an earlier session)`. Each session closed with
  the tab's **X** → `status: Cookie check -> Ok(Cancelled)`. So the
  `auth-session-android-ephemeral-browser-dependent` limitation is exactly
  that: honoured by Chrome, ignored by Fennec F-Droid.

### iOS — iPhone 17 simulator PASSED (machine-driven), 2026-09-23

Rig: iPhone 17 simulator, iOS 26.2 (`xcrun simctl`; the `iPhoneSimulator26.2.sdk`
Xcode toolchain) on a macOS 26.6.2 host. App: the `examples/playground` debug
simulator build (`frust build ios --debug --simulator`, bundle
`it.f0x.playground`, `CFBundleName` = `Runner`). Gate pages served from this
checkout's `plugins/auth-session/gate/` over a scratch https origin on the Mac
(`https://192.168.8.140:8443/`, a throwaway CA trusted on the simulator with
`xcrun simctl keychain add-root-cert`, `Cache-Control: no-store` from the
second cookie visit on). Every tap was scripted: `idb ui tap` reaches the
page's frust buttons and the system consent alert's `Cancel`/`Continue` by
their accessibility labels. Read two independent ways — the app's own console
(`xcrun simctl launch --console-pty`, with a scratch, uncommitted `eprintln!`
of every status write) and screenshots — plus the server's request log, which
records each request's `Cookie` header. The base URL, the start section and
the `Drop` button below were scratch playground edits, reverted before commit.

**Link fix first — the Apple backend had never been linked by Xcode.** The
first build failed:

```
Undefined symbols for architecture arm64:
  "_ASWebAuthenticationSessionErrorDomain", referenced from:
      frust_auth_session::apple::outcome_from in libplayground.a
```

`objc2-authentication-services` declares `#[link(name =
"AuthenticationServices", kind = "framework")]`, which rustc honours when it
links (macOS) but which does not travel through the iOS staticlib into
Xcode's link; the scaffold Runner links Metal/QuartzCore/CoreText/
CoreGraphics/CoreFoundation/UIKit only. `-framework AuthenticationServices`
was added to the playground's `project.pbxproj` (all three configurations)
for the gate; the registry's `IosFramework` contribution now makes `frust
plugin add auth-session` do the same (§ 1).

**(1) Callback** — tap `Callback` (Ephemeral off). Apple's consent alert
appeared — `“Runner” Wants to Use “192.168.8.140” to Sign In` / `This allows
the app and website to share information about you.` with `Cancel` /
`Continue` — then the sheet loaded `callback.html?scheme=frustplay` (server:
`GET /callback.html?scheme=frustplay 200`), redirected and dismissed itself.
Console and labels, verbatim:

```
GATE Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
status: Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
latest deep link: none
is_supported: true
```

`latest deep link` stayed `none`: the redirect was intercepted in-process, so
no deep link was delivered (§ 3).

**(2) Cancel** — (a) with Ephemeral **on**, `Cancel test` presented the sheet
directly, with **no consent alert** (Apple skips it for an ephemeral
session), showing `cookie.html`; closed with the sheet's top-left **X**
(iOS 26 shows a close glyph, not a `Cancel` button) →
`Cancel test -> Ok(Cancelled)`. (b) With Ephemeral **off**, `Cancel test` →
consent alert → **Cancel** → `Cancel test -> Ok(Cancelled)` (`CanceledLogin`;
no sheet was ever shown).

**(3) Busy** — `Busy -> Err(Busy)` was written the instant the button was
tapped; the first session's consent alert → `Continue` → `callback.html`
(server `304`) → `Busy(first) -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`.

**(4) Ephemeral — strict, as Apple documents.** Two instruments agree: the
`Cookie` header on each request the sheet made, and the page's own text.

| Visit | Ephemeral | Request `Cookie` header | Page text |
|---|---|---|---|
| 1 | off | none | `no cookie (set now)` |
| 2 | off | (served from the sheet's cache, no request) | `frustauth=1 (persisted from an earlier session)` |
| 3 | on | none | `no cookie (set now)` |
| 4 | on | none | `no cookie (set now)` |
| 5 | off | (cache) | `frustauth=1 (persisted from an earlier session)` |

The ephemeral sessions saw neither the persisted jar's cookie nor their own
previous run's, and the persisted cookie survived them. Each sheet was closed
with **X** → `Cookie check -> Ok(Cancelled)`. Neither ephemeral visit showed a
consent alert.

**(5) Cancel-before-replace** (the round-1 cap remediation, `68561399`) — a
scratch `Drop` button starts a session against `cookie.html`, drops its future
at once, and 5 s later starts a second session against `callback.html` from a
background thread, while the first is still on screen. (a) Ephemeral: the
first sheet was showing `cookie.html` (server `GET /cookie.html` at
17:59:33Z) when the second start `-cancel`led it and presented its own →
`Drop+restart -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`
(server `GET /callback.html?scheme=frustplay` at 17:59:37Z); no crash, the
page usable afterwards. (b) Non-ephemeral, so the first session's **consent
alert** was still up when the second start landed: the alert was dismissed but
the second `-start` failed —
`Drop+restart -> Err(Platform("com.apple.AuthenticationServices.WebAuthenticationSession 3"))`
(`ASWebAuthenticationSessionErrorCodePresentationContextInvalid`); the Busy
slot was free and the page usable afterwards. Filed as a follow-up: the new
`-start` races the stale alert's dismissal.

**Observations** — the consent alert names the app by `CFBundleName`
(`Runner`, the scaffold's product name), not `CFBundleDisplayName`
(`Playground`); the frust widgets' iOS accessibility frames come back divided
by the 3× scale (a known AccessKit-bounds finding, not this plugin's).

### iOS — iPhone SE (physical) PASSED, 2026-09-23

Device: iPhone SE (2nd generation, `iPhone12,8`), iOS 26.7, network-paired
(no touch injection, no screen capture — every tap was Ed's). App: the
`examples/playground` **release** build (`frust build ios --release -d
<udid>`, same scratch page edits as the simulator leg, same httpbin-hosted
combined page as the macOS leg below), installed and launched with `xcrun
devicectl device process launch --console`, which is the transcript: the
outcome lines below are the app's own console output, not a reading of the
screen; only the cookie page texts are human-observed.

Two rig notes first. The signing identity had expired that morning; after a
new *Apple Development* certificate was issued, the CLI's auto-detected
`DEVELOPMENT_TEAM` was the developer id in the certificate's parentheses
rather than the team in its `OU`, and xcodebuild failed with `No Account for
Team`; `FRUST_IOS_TEAM=<team>` fixed the build (filed against `frust-drive`).
And a console attached to a launch made while the phone was locked recorded
nothing at all — the first pass through the script left no transcript and was
repeated after relaunching with the phone unlocked.

Console, verbatim, in script order:

```
GATE Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
GATE Cancel test -> Ok(Cancelled)                 (sheet closed with X)
GATE Cancel test -> Ok(Cancelled)                 (consent alert Cancel)
GATE Busy -> Err(Busy)
GATE Busy(first) -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
GATE Cookie check -> Ok(Cancelled)                (×4: off, off, on, on)
GATE Drop -> first future dropped (slot released; its sheet still pending on the main queue)
GATE Drop+restart -> starting the second session while the first is still presented
GATE Drop+restart -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
GATE Drop -> first future dropped (slot released; its sheet still pending on the main queue)
GATE Drop+restart -> starting the second session while the first is still presented
GATE Drop+restart -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
GATE page: status: Drop -> first dropped; second start in 5 s | latest deep link: none | is_supported: true
```

**Ephemeral** (four visits, texts read by Ed): with Ephemeral off both visits
showed `frustauth=1 (persisted from an earlier session)` — the persisted jar
already held the cookie from the first, untranscribed pass — and with
Ephemeral on both showed `no cookie (set now)`. Strict on the device as well.

**Cancel-before-replace**: both variants resolved to the callback. In the
non-ephemeral variant the first session's consent alert was on screen when
the restart landed, the alert was replaced by the second session's, and
`Continue` on it completed the second session — **the
`PresentationContextInvalid` race seen on the iOS 26.2 simulator did not
reproduce on the iOS 26.7 device.** `latest deep link` stayed `none`
throughout.

### macOS — PASSED (machine-driven, httpbin-hosted page), 2026-09-23

Host: macOS 26.6.2 (Apple M4), Safari 26. App: the `examples/playground`
desktop debug binary (`cargo build --features frust/devtools`) wrapped in a
hand-made `Playground.app` whose `Info.plist` registers `frustplay` under
`CFBundleURLTypes` — option (b) of the gate card: the playground has no
`macos/` directory and nothing was committed for this — ad-hoc signed and
registered with `lsregister` (`claimed schemes: frustplay:`), run from inside
the bundle so its console stays attached. Driven by devtools `input_tap` for
the page's buttons and System Events clicks for the system UI; read from the
console (`GATE` lines) and, for page text, from Safari's own AppleScript
`text of document`.

**Hosting deviation, recorded as such.** The Mac does not trust the scratch
CA the simulator leg used, and the sheet stops at Safari's `This Connection
Is Not Private` page for that origin (trusting the CA in the login keychain
needs the user's password). The four steps were therefore run against a
scratch **combined** page — `callback.html`'s redirect and `cookie.html`'s
cookie logic in one file, dispatching on the `?p=` value the playground
appends — served from `https://httpbin.org/base64/<urlsafe-b64>?p=` (the
gate README's zero-infra httpbin option; a publicly trusted origin, cookies
on `httpbin.org`). The page's base-URL default was a scratch edit, reverted.

**Presentation.** On macOS the consent alert is a `UserNotificationCenter`
window — `“Playground” Wants to Use “httpbin.org” to Sign In` / `This allows
the app and website to share information about you.` (`Cancel` / `Continue`;
the app name is the bundle's `CFBundleName`) — and the authentication UI is a
**Safari window** (titled after the page, traffic-light close button, no
`Cancel` button of its own), not a sheet over the app window. `is_supported:
true`, `latest deep link: none` throughout (in-process callback, and the
desktop shell delivers no URL-scheme opens anyway).

**(1) Callback** — alert → `Continue` → the Safari window loaded the page,
redirected and closed itself:

```
GATE Callback -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))
```

**(2) Cancel** — (a) `Cancel test` → `Continue` → the Safari window showed the
cookie page (`no cookie (set now)`); closing that window (its close button)
→ `Cancel test -> Ok(Cancelled)`. (b) `Cancel test` → alert `Cancel` →
`Cancel test -> Ok(Cancelled)`.

**(3) Busy** — `Busy -> Err(Busy)` at once; the first session's alert →
`Continue` → `Busy(first) -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`;
its window closed itself.

**(4) Ephemeral — strict.** Page text per visit (each window closed by its
close button → `Cookie check -> Ok(Cancelled)`):

| Visit | Ephemeral | Consent alert | Page text |
|---|---|---|---|
| 1 | off | shown | `no cookie (set now)` |
| 2 | off | shown | `frustauth=1 (persisted from an earlier session)` |
| 3 | on | **none** | `no cookie (set now)` |
| 4 | on | **none** | `no cookie (set now)` |
| 5 | off | shown | `frustauth=1 (persisted from an earlier session)` |

Ephemeral sessions show no consent alert on macOS either. One observation:
the cookie the Cancel-test window (2a) had set was *not* seen by visit 1 —
that window was closed about three seconds after loading — while every later
persisted-jar visit saw the cookie visit 1 set.

**(5) Cancel-before-replace** — same scratch `Drop` button as the simulator
leg. (a) Ephemeral: the first Safari window was showing `no cookie (set now)`
when the restart landed; it closed and the second session resolved →
`Drop+restart -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`.
(b) Non-ephemeral, the first session's consent alert still up when the
restart landed: the alert was replaced by the second session's own alert
(same text); `Continue` on it →
`Drop+restart -> Ok(Callback("frustplay://auth/callback?code=x&state=gate"))`.
**The `PresentationContextInvalid` race seen on the iOS simulator did not
reproduce on macOS.**
