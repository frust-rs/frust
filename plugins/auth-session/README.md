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
    Ok(AuthSessionOutcome::Callback(url)) => {
        // Exchange the authorization code in `url`'s query string for
        // tokens — this crate's job ends at handing back the raw callback
        // URL.
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
  - `ephemeral` maps to `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`,
    advisory to the browser (a browser that doesn't support it opens a normal
    tab instead of refusing).
  - No installed browser capable of a Custom Tab resolves
    `AuthSessionError::NoHandler`.
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

## 4. One-session contract

At most one `AuthSession` runs at a time, process-wide: starting a second one
while the first is still live resolves immediately to
`AuthSessionError::Busy` rather than queuing or replacing it (an in-app
browser tab is a modal, single-instance UI surface on every backend
platform). The slot frees the moment the live session resolves — by outcome,
by error, or by a backend module failing without ever calling back — so a
stuck session can never wedge every later one behind it.

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

Owed — no device or simulator run has exercised either backend yet (see § 3
for what each does). A follow-on task records the Android Custom Tabs and
Apple `ASWebAuthenticationSession` device-gate transcripts; see
[PLUGINS_DEVELOPMENT.md](../../docs/PLUGINS_DEVELOPMENT.md)'s Auth-session
manual test for the script it will run.
